#!/usr/bin/env bash
# Regression test proving the native artifact placement loop lives INSIDE
# the INPUT_PUBLISH guard (SPEC-002 AC-003's "not merely somewhere after
# NATIVE_MATRIX computation" clause, and AC-004's second sentence).
#
# test_native_artifact_placement.sh extracts the loop body in isolation
# (between its own ::group::/::endgroup:: markers), so it would keep
# passing even if the loop were moved outside the guard entirely. This
# test instead extracts from the guard's own `if` line through the loop's
# ::endgroup:: -- inclusive of the guard condition itself -- so it fails
# if the loop is ever hoisted above (or the guard condition weakened
# around) that `if` line.
set -u
ACTION_YML="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/action.yml"

# The range starts at the guard's exact `if` line and ends at the FIRST
# ::endgroup:: after it -- which, if the loop is still correctly placed
# immediately inside the guard, is the loop's own closing marker. A
# missing/moved guard or loop means this range no longer captures both
# together, and the appended `fi` below then either closes nothing
# (syntax error) or closes a guard with an empty body (no gh calls even
# when INPUT_PUBLISH=true).
extract_guarded_snippet() {
  sed -n '/if \[\[ -n "\$INPUT_PUBLISH" && "\$INPUT_PUBLISH" != "false" \]\]; then/,/::endgroup::/p' "$ACTION_YML"
  echo "fi"
}

run_case() {
  local native_matrix="$1"
  local input_publish="$2"
  local calls_file tmp_script workdir
  calls_file="$(mktemp)"
  workdir="$(mktemp -d)"
  tmp_script="$(mktemp)"
  {
    echo "gh() { echo \"\$*\" >> '${calls_file}'; return 0; }"
    echo "mkdir() { command mkdir \"\$@\"; }"
    echo "cd '${workdir}'"
    echo "NATIVE_MATRIX=$(printf '%q' "$native_matrix")"
    echo "GITHUB_RUN_ID=999"
    echo "INPUT_PUBLISH=$(printf '%q' "$input_publish")"
    extract_guarded_snippet
  } > "$tmp_script"
  bash "$tmp_script"
  local code=$?
  cat "$calls_file"
  rm -f "$tmp_script" "$calls_file"
  rm -rf "$workdir"
  return $code
}

fail=0
MATRIX='[{"artifactName":"pkg-a-darwin-arm64","packageDir":"packages/pkg-a"}]'

# Positive control: INPUT_PUBLISH=true must reach the loop and call gh.
out=$(run_case "$MATRIX" "true"); code=$?
if [[ $code -ne 0 ]] || [[ "$out" != *"run download 999 --name pkg-a-darwin-arm64"* ]]; then
  echo "FAIL positive control (INPUT_PUBLISH=true must place artifacts): code=$code out=$out"; fail=1
else
  echo "PASS positive control (INPUT_PUBLISH=true places artifacts)"
fi

# AC-003/AC-004: INPUT_PUBLISH unset -> the guard must skip the loop
# entirely. If the loop were ever hoisted above this guard's `if` line,
# this case would start calling gh and fail.
out=$(run_case "$MATRIX" ""); code=$?
if [[ $code -ne 0 ]] || [[ -n "$out" ]]; then
  echo "FAIL AC-003/AC-004 (INPUT_PUBLISH unset must skip placement): code=$code out=$out"; fail=1
else
  echo "PASS AC-003/AC-004 (INPUT_PUBLISH unset skips placement)"
fi

# INPUT_PUBLISH="false" -> same guard, same expectation.
out=$(run_case "$MATRIX" "false"); code=$?
if [[ $code -ne 0 ]] || [[ -n "$out" ]]; then
  echo "FAIL AC-003/AC-004 (INPUT_PUBLISH=false must skip placement): code=$code out=$out"; fail=1
else
  echo "PASS AC-003/AC-004 (INPUT_PUBLISH=false skips placement)"
fi

exit $fail
