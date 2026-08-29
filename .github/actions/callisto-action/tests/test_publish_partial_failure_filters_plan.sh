#!/usr/bin/env bash
# Regression test: when `callisto publish` reports one package failed and
# others succeeded, the step must still tag/release the successes (using
# `callisto filter-plan` to narrow PLAN_JSON down to what actually
# published), and must fail the job overall afterward -- not before tagging
# the successes, and not silently swallow the failure either. Before this
# fix, `callisto publish` ran under `set -e` with no exit-code capture: any
# failure killed the step immediately, so already-succeeded siblings in the
# same run never got tagged or released, on every retry, until the failing
# package was fixed or excluded.
#
# Extracts the exact run-body text from the `hasChangesets=false` line
# through the end of the step, so this test always exercises the file's
# current real logic.
set -u
ACTION_YML="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/action.yml"

extract_snippet() {
  sed -n '/echo "hasChangesets=false" >> \$GITHUB_OUTPUT/,$p' "$ACTION_YML" | sed '$d'
}

run_case() {
  local plan_json="$1"
  local report_json="$2"
  local publish_exit="$3"
  local filtered_plan_json="$4"
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
    echo "    publish) echo $(printf '%q' "$report_json"); return $publish_exit ;;"
    echo "    filter-plan) echo $(printf '%q' "$filtered_plan_json") ;;"
    echo "    tag) echo '{\"schemaVersion\":1,\"tags\":[]}' ;;"
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

PLAN='{"schemaVersion":1,"rustCrates":[{"name":"pkg-a","version":"1.0.0","publishTo":"cratesIo"},{"name":"pkg-b","version":"1.0.0","publishTo":"cratesIo"}],"npmPlatformPackages":[],"npmMainPackages":[],"releases":[{"package":"pkg-a","tagName":"pkg-a@1.0.0","sha":"a","isPrerelease":false}]}'
REPORT='{"schemaVersion":1,"attempts":[{"package":"cargo:pkg-a","version":"1.0.0","status":"published"},{"package":"cargo:pkg-b","version":"1.0.0","status":"failed","errorKind":"other","error":"boom"}]}'
# filter-plan's own behavior is independently unit-tested in callisto-graph;
# here it's stubbed to return exactly what a real filter-plan call would
# produce for REPORT above (pkg-b dropped, pkg-a and its release kept).
FILTERED='{"schemaVersion":1,"rustCrates":[{"name":"pkg-a","version":"1.0.0","publishTo":"cratesIo"}],"npmPlatformPackages":[],"npmMainPackages":[],"releases":[{"package":"pkg-a","tagName":"pkg-a@1.0.0","sha":"a","isPrerelease":false}]}'

out=$(run_case "$PLAN" "$REPORT" 1 "$FILTERED"); code=$?

if [[ $code -eq 0 ]]; then
  echo "FAIL: the step must still fail overall when callisto publish reported a failure: code=$code out=$out"; fail=1
else
  echo "PASS: step fails overall when callisto publish reports a failure"
fi

if [[ "$out" != *"CALL:filter-plan"* ]]; then
  echo "FAIL: filter-plan must be called to narrow the plan to actual successes: out=$out"; fail=1
else
  echo "PASS: filter-plan is called after publish"
fi

if [[ "$out" != *"CALL:tag --plan"* ]]; then
  echo "FAIL: the already-succeeded package must still be tagged despite the sibling's failure: out=$out"; fail=1
else
  echo "PASS: tag still runs for the succeeded package"
fi

if [[ "$out" != *"CALL:gh release create pkg-a@1.0.0"* ]]; then
  echo "FAIL: the already-succeeded package must still get a GitHub Release despite the sibling's failure: out=$out"; fail=1
else
  echo "PASS: release is still created for the succeeded package"
fi

# The published output must reflect the FILTERED plan, not the original
# pre-publish plan -- an operator reading publishedPackages must see what
# actually shipped. Isolated to the ---OUTPUT--- section (the real
# GITHUB_OUTPUT file content) specifically, since the surrounding debug
# recap (---CALLS---) legitimately echoes pkg-b as part of filter-plan's
# own --plan/--report arguments.
output_section=$(sed -n '/^---OUTPUT---$/,/^---CALLS---$/p' <<< "$out")
if [[ "$output_section" != *'"pkg-a"'* ]] || [[ "$output_section" == *'"pkg-b"'* ]]; then
  echo "FAIL: publishedPackages output must be the filtered plan (pkg-a only, not pkg-b): output_section=$output_section"; fail=1
else
  echo "PASS: publishedPackages output reflects the filtered plan"
fi

# Sequencing: the ::error:: for the overall publish failure must come from
# callisto reporting failures, AFTER tag/release ran for the successes --
# not a bare early exit before them. Both markers checked here are printed
# live to real stdout by the snippet itself (not the test harness's own
# ---CALLS--- recap), so their relative order reflects true execution order.
release_line=$(grep -n "Creating GitHub Release for tag: pkg-a@1.0.0" <<< "$out" | head -1 | cut -d: -f1)
error_line=$(grep -n "::error::callisto publish reported" <<< "$out" | head -1 | cut -d: -f1)
if [[ -z "$release_line" ]] || [[ -z "$error_line" ]] || [[ "$release_line" -ge "$error_line" ]]; then
  echo "FAIL: tag/release must run before the overall publish-failure error is raised: release_line=$release_line error_line=$error_line"; fail=1
else
  echo "PASS: tag/release for successes run before the job is failed"
fi

exit $fail
