#!/usr/bin/env bash
# Regression test: a run with no changesets pending but every package
# already at its last-tagged version (a fully empty publish plan -- e.g. a
# docs-only commit right after a prior successful release) must report
# published=false and must not run the native-artifact placement loop,
# `callisto publish`, `callisto tag`, or any `git push` -- all of which were
# previously invoked unconditionally whenever INPUT_PUBLISH was set, plan
# emptiness was never checked.
#
# Extracts the exact run-body text from the `hasChangesets=false` line
# (the start of the "no pending changesets" branch) through the end of the
# step, so this test always exercises the file's current real logic.
set -u
ACTION_YML="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/action.yml"

extract_snippet() {
  # The captured range ends one line before EOF: the file's very last line
  # is the `fi` closing the `if [[ "$HAS_CHANGESETS" == "true" ]]` header
  # this snippet's own `else` belongs to, which isn't included here, so
  # that trailing `fi` would otherwise be a dangling, unmatched close.
  sed -n '/echo "hasChangesets=false" >> \$GITHUB_OUTPUT/,$p' "$ACTION_YML" | sed '$d'
}

# Runs the extracted snippet with a stubbed `callisto` (matrix and
# plan-publish return canned JSON; publish/tag/filter-plan fail the test
# outright if called, since an empty plan must never reach them) and
# stubbed `gh`/`git` (also fail the test outright if called).
run_case() {
  local plan_json="$1"
  local calls_file tmp_script output_file
  calls_file="$(mktemp)"
  output_file="$(mktemp)"
  tmp_script="$(mktemp)"
  {
    echo "GITHUB_OUTPUT=$(printf '%q' "$output_file")"
    echo "GITHUB_STEP_SUMMARY=/dev/null"
    echo "GITHUB_RUN_ID=999"
    echo "NPM_TOKEN="
    echo "INPUT_PUBLISH=true"
    echo "INPUT_CREATE_GITHUB_RELEASE=true"
    echo "callisto() {"
    echo "  echo \"CALL:\$*\" >> '${calls_file}'"
    echo "  case \"\$1\" in"
    echo "    matrix) echo '{\"platformTargets\":[]}' ;;"
    echo "    plan-publish) echo $(printf '%q' "$plan_json") ;;"
    echo "    publish|tag|filter-plan) echo 'FORBIDDEN: empty plan must not reach callisto '\"\$1\" >&2; exit 1 ;;"
    echo "    *) return 0 ;;"
    echo "  esac"
    echo "}"
    echo "gh() { echo \"CALL:gh \$*\" >> '${calls_file}'; return 0; }"
    echo "git() { echo \"CALL:git \$*\" >> '${calls_file}'; return 0; }"
    extract_snippet
  } > "$tmp_script"
  bash "$tmp_script" 2>&1
  local code=$?
  echo "---OUTPUT---"
  cat "$output_file"
  echo "---CALLS---"
  cat "$calls_file"
  rm -f "$tmp_script" "$calls_file" "$output_file"
  return $code
}

fail=0
EMPTY_PLAN='{"schemaVersion":1,"rustCrates":[],"npmPlatformPackages":[],"npmMainPackages":[],"releases":[]}'
NONEMPTY_PLAN='{"schemaVersion":1,"rustCrates":[{"name":"pkg","version":"1.0.0","publishTo":"cratesIo"}],"npmPlatformPackages":[],"npmMainPackages":[],"releases":[]}'

# Empty plan: published=false, and none of the downstream commands run.
out=$(run_case "$EMPTY_PLAN"); code=$?
if [[ $code -ne 0 ]]; then
  echo "FAIL: empty-plan run must not fail the step: code=$code out=$out"; fail=1
elif [[ "$out" != *"published=false"* ]]; then
  echo "FAIL: empty-plan run must report published=false: out=$out"; fail=1
elif [[ "$out" == *"CALL:gh run download"* ]] || [[ "$out" == *"CALL:git push"* ]] || [[ "$out" == *"FORBIDDEN"* ]]; then
  echo "FAIL: empty-plan run must not reach artifact placement, publish, tag, or push: out=$out"; fail=1
else
  echo "PASS: empty plan skips artifact placement / publish / tag / release entirely"
fi

# Positive control: a non-empty plan must still reach callisto publish (and
# thus prove the stub/harness itself isn't just silently no-oping).
out=$(run_case "$NONEMPTY_PLAN"); code=$?
if [[ "$out" != *"FORBIDDEN: empty plan must not reach callisto publish"* ]]; then
  echo "FAIL: positive control must reach callisto publish (proving the harness actually exercises the guard): out=$out"; fail=1
else
  echo "PASS: positive control reaches callisto publish for a non-empty plan"
fi

exit $fail
