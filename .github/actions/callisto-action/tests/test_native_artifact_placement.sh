#!/usr/bin/env bash
# Regression test for the native artifact placement loop in action.yml
# (SPEC-002 AC-003, AC-004, AC-005, AC-006, AC-010). Extracts the exact
# run-body text between the step's ::group::Placing Native Build Artifacts
# marker and its matching ::endgroup:: marker directly out of action.yml, so
# this test always exercises the file's current real logic -- it cannot
# drift from it.
set -u
ACTION_YML="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/action.yml"

extract_snippet() {
  sed -n '/::group::Placing Native Build Artifacts/,/::endgroup::/p' "$ACTION_YML" | sed '1d;$d'
}

# Runs the extracted snippet with the given NATIVE_MATRIX and GITHUB_RUN_ID,
# stubbing gh, mkdir, and jq to record invocations into files; jq delegates
# to the real `command jq` so the snippet's actual jq logic still executes.
run_case() {
  local native_matrix="$1"
  local run_id="$2"
  local gh_exit="${3:-0}"
  local calls_file mkdir_file jq_file tmp_script workdir
  calls_file="$(mktemp)"
  mkdir_file="$(mktemp)"
  jq_file="$(mktemp)"
  workdir="$(mktemp -d)"
  tmp_script="$(mktemp)"
  {
    echo "gh() { echo \"\$*\" >> '${calls_file}'; return ${gh_exit}; }"
    echo "mkdir() { echo \"\$*\" >> '${mkdir_file}'; command mkdir \"\$@\"; }"
    echo "jq() { echo \"\$*\" >> '${jq_file}'; command jq \"\$@\"; }"
    echo "cd '${workdir}'"
    echo "NATIVE_MATRIX=$(printf '%q' "$native_matrix")"
    echo "GITHUB_RUN_ID=$(printf '%q' "$run_id")"
    extract_snippet
  } > "$tmp_script"
  bash "$tmp_script"
  local code=$?
  echo "---GH_CALLS---"
  cat "$calls_file"
  echo "---MKDIR_CALLS---"
  cat "$mkdir_file"
  echo "---JQ_CALLS---"
  cat "$jq_file"
  rm -f "$tmp_script" "$calls_file" "$mkdir_file" "$jq_file"
  rm -rf "$workdir"
  return $code
}

fail=0

# AC-003 / AC-010: two entries -> gh run download invoked once per entry,
# pinned to GITHUB_RUN_ID, with each entry's own artifactName/packageDir.
MATRIX='[{"artifactName":"pkg-a-darwin-arm64","packageDir":"packages/pkg-a"},{"artifactName":"pkg-b-darwin-arm64","packageDir":"packages/pkg-b"}]'
out=$(run_case "$MATRIX" "999"); code=$?
if [[ $code -ne 0 ]] \
  || [[ "$out" != *"run download 999 --name pkg-a-darwin-arm64 --dir packages/pkg-a"* ]] \
  || [[ "$out" != *"run download 999 --name pkg-b-darwin-arm64 --dir packages/pkg-b"* ]]; then
  echo "FAIL AC-003/AC-010 (two entries placed): code=$code out=$out"; fail=1
else
  echo "PASS AC-003/AC-010"
fi

# AC-004: empty array -> zero gh invocations, step still succeeds.
out=$(run_case "[]" "999"); code=$?
gh_calls=$(echo "$out" | sed -n '/---GH_CALLS---/,/---MKDIR_CALLS---/p' | sed '1d;$d')
if [[ $code -ne 0 ]] || [[ -n "$gh_calls" ]]; then
  echo "FAIL AC-004 (empty nativeMatrix): code=$code out=$out"; fail=1
else
  echo "PASS AC-004"
fi

# AC-005: empty packageDir -> DIR normalized to "." for both mkdir -p and
# gh run download --dir.
MATRIX='[{"artifactName":"root-pkg-linux-x64-gnu","packageDir":""}]'
out=$(run_case "$MATRIX" "42"); code=$?
if [[ $code -ne 0 ]] \
  || [[ "$out" != *"run download 42 --name root-pkg-linux-x64-gnu --dir ."* ]] \
  || [[ "$out" != *"-p ."* ]]; then
  echo "FAIL AC-005 (empty packageDir normalizes to .): code=$code out=$out"; fail=1
else
  echo "PASS AC-005"
fi

# AC-006: gh run download fails -> step exits non-zero, error names the
# artifact and DIR, and no further entries are attempted (loop stops).
MATRIX='[{"artifactName":"missing-pkg-linux-x64-gnu","packageDir":"packages/missing"},{"artifactName":"never-reached-pkg-linux-x64-gnu","packageDir":"packages/never"}]'
out=$(run_case "$MATRIX" "7" "1"); code=$?
if [[ $code -eq 0 ]] \
  || [[ "$out" != *"::error::"*"missing-pkg-linux-x64-gnu"* ]] \
  || [[ "$out" != *"packages/missing"* ]] \
  || [[ "$out" == *"never-reached-pkg-linux-x64-gnu"* ]]; then
  echo "FAIL AC-006 (download failure halts step with named error): code=$code out=$out"; fail=1
else
  echo "PASS AC-006"
fi

# jq-spawn-count: 3-entry matrix -> jq invoked exactly once total (a single
# upfront extraction call), not once for the array plus twice per entry.
MATRIX='[{"artifactName":"a","packageDir":"pkgs/a"},{"artifactName":"b","packageDir":"pkgs/b"},{"artifactName":"c","packageDir":"pkgs/c"}]'
out=$(run_case "$MATRIX" "1"); code=$?
jq_calls=$(echo "$out" | sed -n '/---JQ_CALLS---/,$p' | sed '1d')
jq_call_count=$(echo "$jq_calls" | grep -c . || true)
if [[ $code -ne 0 ]] || [[ "$jq_call_count" -ne 1 ]]; then
  echo "FAIL jq-spawn-count (expected exactly 1 jq invocation, got $jq_call_count): code=$code out=$out"; fail=1
else
  echo "PASS jq-spawn-count"
fi

# packageDir JSON null: preserved byte-for-byte as the literal string "null"
# for both mkdir -p and gh run download --dir (pre-existing edge behavior;
# the @tsv refactor must not silently turn this into DIR=".").
MATRIX='[{"artifactName":"x","packageDir":null}]'
out=$(run_case "$MATRIX" "5"); code=$?
if [[ $code -ne 0 ]] \
  || [[ "$out" != *"run download 5 --name x --dir null"* ]] \
  || [[ "$out" != *"-p null"* ]]; then
  echo "FAIL packageDir-null (JSON null preserved as literal 'null' dir): code=$code out=$out"; fail=1
else
  echo "PASS packageDir-null"
fi

# packageDir key entirely absent: jq's missing-key lookup also yields null,
# so this must behave identically to the explicit-null case above.
MATRIX='[{"artifactName":"y"}]'
out=$(run_case "$MATRIX" "6"); code=$?
if [[ $code -ne 0 ]] \
  || [[ "$out" != *"run download 6 --name y --dir null"* ]] \
  || [[ "$out" != *"-p null"* ]]; then
  echo "FAIL packageDir-absent (missing key treated same as JSON null): code=$code out=$out"; fail=1
else
  echo "PASS packageDir-absent"
fi

exit $fail
