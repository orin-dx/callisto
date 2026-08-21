#!/usr/bin/env bash
# Regression test for the "Creating GitHub Release Entries" step in action.yml
# (SPEC-005 AC-008, AC-009, AC-010, AC-015). Extracts the exact run-body text
# between the step's ::group::5. Creating GitHub Release Entries marker and
# its matching ::endgroup:: marker directly out of action.yml, so this test
# always exercises the file's current real logic -- it cannot drift from it.
set -u
ACTION_YML="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/action.yml"

extract_snippet() {
  sed -n '/::group::5\. Creating GitHub Release Entries/,/::endgroup::/p' "$ACTION_YML" | sed '1d;$d'
}

# Runs the extracted snippet with the given PLAN_JSON, stubbing gh to record
# each invocation's arguments (one per line) into $GH_CALLS_FILE. jq is real.
run_case() {
  local plan_json="$1"
  local calls_file
  calls_file="$(mktemp)"
  local tmp_script
  tmp_script="$(mktemp)"
  {
    echo "gh() { echo \"\$*\" >> '${calls_file}'; return 0; }"
    echo "PLAN_JSON=$(printf '%q' "$plan_json")"
    extract_snippet
  } > "$tmp_script"
  bash "$tmp_script"
  local code=$?
  cat "$calls_file"
  rm -f "$tmp_script" "$calls_file"
  return $code
}

fail=0

# AC-009 / AC-010 case 1: isPrerelease true, tag has no alpha/beta/rc/pre/next
# substring (the AC-005 PEP 440 regression fixture) -- must still get --prerelease.
PLAN='{"releases":[{"package":"pkg","tagName":"pkg@1.2.3a1","sha":"deadbeef","isPrerelease":true}]}'
out=$(run_case "$PLAN"); code=$?
if [[ $code -ne 0 ]] || [[ "$out" != *"pkg@1.2.3a1 --title pkg@1.2.3a1 --generate-notes --prerelease"* ]]; then
  echo "FAIL AC-009/AC-010 case1 (isPrerelease true, no marker substring): code=$code out=$out"; fail=1
else
  echo "PASS AC-009/AC-010 case1"
fi

# AC-009 / AC-010 case 2: isPrerelease false -- no --prerelease flag.
PLAN='{"releases":[{"package":"pkg","tagName":"pkg@1.2.3","sha":"deadbeef","isPrerelease":false}]}'
out=$(run_case "$PLAN"); code=$?
if [[ $code -ne 0 ]] || [[ "$out" != *"pkg@1.2.3 --title pkg@1.2.3 --generate-notes"* ]] || [[ "$out" == *"--prerelease"* ]]; then
  echo "FAIL AC-009/AC-010 case2 (isPrerelease false): code=$code out=$out"; fail=1
else
  echo "PASS AC-009/AC-010 case2"
fi

# AC-008: the alpha/beta/rc/pre/next tag-string regex is gone.
if grep -q '\-(alpha|beta|rc|pre|next)' "$ACTION_YML"; then
  echo "FAIL AC-008: tag-string prerelease regex still present in action.yml"; fail=1
else
  echo "PASS AC-008"
fi

# AC-015 case: missing isPrerelease key -- must fail before any gh call.
PLAN='{"releases":[{"package":"pkg","tagName":"pkg@1.0.0","sha":"deadbeef"}]}'
out=$(run_case "$PLAN"); code=$?
if [[ $code -eq 0 ]] || [[ "$out" == *"gh release create"* ]] || [[ "$out" == *"pkg@1.0.0 --title"* ]]; then
  echo "FAIL AC-015 (missing key): code=$code out=$out"; fail=1
else
  echo "PASS AC-015 (missing key)"
fi

# AC-015 case: non-boolean isPrerelease value -- must fail before any gh call.
PLAN='{"releases":[{"package":"pkg","tagName":"pkg@1.0.0","sha":"deadbeef","isPrerelease":"true"}]}'
out=$(run_case "$PLAN"); code=$?
if [[ $code -eq 0 ]] || [[ "$out" == *"pkg@1.0.0 --title"* ]]; then
  echo "FAIL AC-015 (non-boolean): code=$code out=$out"; fail=1
else
  echo "PASS AC-015 (non-boolean)"
fi

# AC-015 case: null isPrerelease value -- must fail before any gh call.
PLAN='{"releases":[{"package":"pkg","tagName":"pkg@1.0.0","sha":"deadbeef","isPrerelease":null}]}'
out=$(run_case "$PLAN"); code=$?
if [[ $code -eq 0 ]] || [[ "$out" == *"pkg@1.0.0 --title"* ]]; then
  echo "FAIL AC-015 (null): code=$code out=$out"; fail=1
else
  echo "PASS AC-015 (null)"
fi

# AC-009: two entries with the identical tag string are processed independently
# on their own entry's isPrerelease value.
PLAN='{"releases":[{"package":"a","tagName":"dup@1.0.0","sha":"aaaa","isPrerelease":true},{"package":"b","tagName":"dup@1.0.0","sha":"bbbb","isPrerelease":false}]}'
out=$(run_case "$PLAN"); code=$?
count_prerelease=$(grep -c -- "--prerelease" <<< "$out")
count_total=$(grep -c "dup@1.0.0 --title" <<< "$out")
if [[ $code -ne 0 ]] || [[ "$count_total" -ne 2 ]] || [[ "$count_prerelease" -ne 1 ]]; then
  echo "FAIL AC-009 (duplicate tag, independent entries): code=$code count_total=$count_total count_prerelease=$count_prerelease out=$out"; fail=1
else
  echo "PASS AC-009 (duplicate tag, independent entries)"
fi

exit $fail
