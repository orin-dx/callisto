#!/usr/bin/env bash
# Regression test for the "Inspecting Workspace Status" step's exit-code
# branching in action.yml (AC-01, AC-02, AC-04). Extracts the exact run-body
# text between the step's ::group::1. Inspecting Workspace Status marker and
# its matching ::endgroup:: marker directly out of action.yml, so this test
# always exercises the file's current real logic -- it cannot drift from it.
set -u
ACTION_YML="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/action.yml"

extract_snippet() {
  sed -n '/::group::1\. Inspecting Workspace Status/,/::endgroup::/p' "$ACTION_YML" | sed '1d;$d'
}

run_case() {
  local stub_exit="$1"
  local tmp_script
  tmp_script="$(mktemp)"
  {
    echo "callisto() { return ${stub_exit}; }"
    extract_snippet
    echo 'echo "__HAS_CHANGESETS=${HAS_CHANGESETS-<unset>}__"'
  } > "$tmp_script"
  bash "$tmp_script"
  local code=$?
  rm -f "$tmp_script"
  return $code
}

fail=0

# AC-01: exit 1 -> HAS_CHANGESETS=true, step must not fail.
out=$(run_case 1); code=$?
if [[ $code -ne 0 ]] || [[ "$out" != *"__HAS_CHANGESETS=true__"* ]]; then
  echo "FAIL AC-01: exit 1 case: code=$code out=$out"; fail=1
else
  echo "PASS AC-01"
fi

# AC-02: exit 2 -> HAS_CHANGESETS=false, step must not fail.
out=$(run_case 2); code=$?
if [[ $code -ne 0 ]] || [[ "$out" != *"__HAS_CHANGESETS=false__"* ]]; then
  echo "FAIL AC-02: exit 2 case: code=$code out=$out"; fail=1
else
  echo "PASS AC-02"
fi

# AC-04: exit 0 -> step must fail (non-zero), HAS_CHANGESETS never read.
out=$(run_case 0); code=$?
if [[ $code -eq 0 ]] || [[ "$out" == *"__HAS_CHANGESETS=true__"* ]] || [[ "$out" == *"__HAS_CHANGESETS=false__"* ]]; then
  echo "FAIL AC-04 (exit 0): code=$code out=$out"; fail=1
else
  echo "PASS AC-04 (exit 0)"
fi

# AC-04: exit 127 -> same requirement, a different unrecognised code.
out=$(run_case 127); code=$?
if [[ $code -eq 0 ]] || [[ "$out" == *"__HAS_CHANGESETS=true__"* ]] || [[ "$out" == *"__HAS_CHANGESETS=false__"* ]]; then
  echo "FAIL AC-04 (exit 127): code=$code out=$out"; fail=1
else
  echo "PASS AC-04 (exit 127)"
fi

exit $fail
