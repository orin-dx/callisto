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
# stubbing gh and mkdir to record invocations into files; jq is real.
run_case() {
  local native_matrix="$1"
  local run_id="$2"
  local gh_exit="${3:-0}"
  local calls_file mkdir_file tmp_script workdir
  calls_file="$(mktemp)"
  mkdir_file="$(mktemp)"
  workdir="$(mktemp -d)"
  tmp_script="$(mktemp)"
  {
    echo "gh() { echo \"\$*\" >> '${calls_file}'; return ${gh_exit}; }"
    echo "mkdir() { echo \"\$*\" >> '${mkdir_file}'; command mkdir \"\$@\"; }"
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
  rm -f "$tmp_script" "$calls_file" "$mkdir_file"
  rm -rf "$workdir"
  return $code
}

fail=0

# AC-003 / AC-010: two entries -> gh run download invoked once per entry,
# pinned to GITHUB_RUN_ID, with each entry's own artifactName/packageDir.
MATRIX='[{"artifactName":"native-pkg-a-aarch64-apple-darwin","packageDir":"packages/pkg-a"},{"artifactName":"native-pkg-b-aarch64-apple-darwin","packageDir":"packages/pkg-b"}]'
out=$(run_case "$MATRIX" "999"); code=$?
if [[ $code -ne 0 ]] \
  || [[ "$out" != *"run download 999 --name native-pkg-a-aarch64-apple-darwin --dir packages/pkg-a"* ]] \
  || [[ "$out" != *"run download 999 --name native-pkg-b-aarch64-apple-darwin --dir packages/pkg-b"* ]]; then
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
MATRIX='[{"artifactName":"native-root-pkg-x86_64-unknown-linux-gnu","packageDir":""}]'
out=$(run_case "$MATRIX" "42"); code=$?
if [[ $code -ne 0 ]] \
  || [[ "$out" != *"run download 42 --name native-root-pkg-x86_64-unknown-linux-gnu --dir ."* ]] \
  || [[ "$out" != *"-p ."* ]]; then
  echo "FAIL AC-005 (empty packageDir normalizes to .): code=$code out=$out"; fail=1
else
  echo "PASS AC-005"
fi

# AC-006: gh run download fails -> step exits non-zero, error names the
# artifact and DIR, and no further entries are attempted (loop stops).
MATRIX='[{"artifactName":"native-missing-x86_64-unknown-linux-gnu","packageDir":"packages/missing"},{"artifactName":"native-never-reached-x86_64-unknown-linux-gnu","packageDir":"packages/never"}]'
out=$(run_case "$MATRIX" "7" "1"); code=$?
if [[ $code -eq 0 ]] \
  || [[ "$out" != *"::error::"*"native-missing-x86_64-unknown-linux-gnu"* ]] \
  || [[ "$out" != *"packages/missing"* ]] \
  || [[ "$out" == *"native-never-reached-x86_64-unknown-linux-gnu"* ]]; then
  echo "FAIL AC-006 (download failure halts step with named error): code=$code out=$out"; fail=1
else
  echo "PASS AC-006"
fi

exit $fail
