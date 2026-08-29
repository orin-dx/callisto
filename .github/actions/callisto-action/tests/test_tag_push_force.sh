#!/usr/bin/env bash
# Regression tests for the "4. Tagging Release Commits & Updating Major
# Version Pointer Aliases" step in action.yml.
#
# History: PR #16 (2026-08-27) found a plain `git push origin --tags` can
# never move an already-existing remote tag, so the floating major-version
# alias (e.g. `pkg@0`, force-moved LOCALLY via `git tag -f`) silently failed
# to reach the remote on every release after the first -- fixed by forcing
# the tags push (PR #18). That blanket `--tags --force` then turned out to
# be its OWN bug: it force-pushes every local tag, immutable per-version
# release tags included, discarding git's own protection against silently
# overwriting an already-published tag. This step now scopes the force-push
# to only the tags `callisto tag`'s report marks isFloatingMajor -- an
# immutable release tag is pushed plainly, so a genuine conflict there is
# still caught by git's non-fast-forward check instead of being silently
# clobbered.
#
# Extracts the step's exact run-body text directly out of action.yml, so
# this test always exercises the file's current real logic.
set -u
ACTION_YML="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/action.yml"

extract_snippet() {
  sed -n '/::group::4\. Tagging Release Commits/,/::endgroup::/p' "$ACTION_YML" | sed '1d;$d'
}

# Runs the extracted snippet with the given PLAN_JSON, stubbing `callisto`
# (its `tag` subcommand returns $tag_report_json with exit $tag_exit) and
# `git` (records every invocation, one per line, into $GIT_CALLS_FILE).
run_case() {
  local plan_json="$1"
  local tag_report_json="$2"
  local tag_exit="$3"
  local calls_file tmp_script
  calls_file="$(mktemp)"
  tmp_script="$(mktemp)"
  {
    echo "callisto() { if [[ \"\$1\" == \"tag\" ]]; then echo $(printf '%q' "$tag_report_json"); return $tag_exit; fi; }"
    echo "git() { echo \"\$*\" >> '${calls_file}'; return 0; }"
    echo "PLAN_JSON=$(printf '%q' "$plan_json")"
    extract_snippet
  } > "$tmp_script"
  local stderr_file
  stderr_file="$(mktemp)"
  bash "$tmp_script" 2>"$stderr_file"
  local code=$?
  cat "$calls_file"
  echo "---STDERR---"
  cat "$stderr_file"
  rm -f "$tmp_script" "$calls_file" "$stderr_file"
  return $code
}

fail=0
PLAN='{"rustCrates":[],"npmPlatformPackages":[],"npmMainPackages":[],"releases":[{"package":"pkg","tagName":"pkg@1.0.0","sha":"a","isPrerelease":false}]}'

# Case 1: a mixed batch (one immutable release tag, one floating major
# alias) -- the immutable tag must be pushed WITHOUT --force; the floating
# alias must be pushed WITH --force.
REPORT='{"schemaVersion":1,"tags":[{"package":"pkg","tagName":"pkg@1.0.0","sha":"a","alreadyExisted":false,"isFloatingMajor":false},{"package":"pkg","tagName":"pkg@1","sha":"a","alreadyExisted":true,"isFloatingMajor":true}]}'
out=$(run_case "$PLAN" "$REPORT" 0); code=$?
if [[ $code -ne 0 ]] || [[ "$out" != *"push origin pkg@1.0.0"* ]] || [[ "$out" == *"push origin --force pkg@1.0.0"* ]]; then
  echo "FAIL: immutable release tag must be pushed without --force: code=$code out=$out"; fail=1
else
  echo "PASS: immutable release tag pushed without --force"
fi
if [[ "$out" != *"push origin --force pkg@1"* ]]; then
  echo "FAIL: floating major-version alias must be force-pushed: out=$out"; fail=1
else
  echo "PASS: floating major-version alias force-pushed"
fi

# Case 2: `callisto tag` fails -- the failure must surface via ::error::,
# not be swallowed, and no git push must be attempted at all (nothing to
# push if tagging itself failed).
out=$(run_case "$PLAN" '{"error":"boom"}' 1); code=$?
if [[ "$out" != *"::error::"* ]]; then
  echo "FAIL: a callisto tag failure must surface via ::error::, not be silently swallowed: out=$out"; fail=1
else
  echo "PASS: callisto tag failure surfaces via ::error::"
fi
if echo "$out" | grep -q "^push origin"; then
  echo "FAIL: no git push must happen when callisto tag itself failed: out=$out"; fail=1
else
  echo "PASS: no git push attempted when callisto tag failed"
fi

# Case 3: only an immutable release tag, no floating alias this run (a
# package whose major version doesn't change) -- must push the release tag
# and must not error out attempting to push an empty floating-tags list.
REPORT='{"schemaVersion":1,"tags":[{"package":"pkg","tagName":"pkg@1.0.1","sha":"a","alreadyExisted":false,"isFloatingMajor":false}]}'
out=$(run_case "$PLAN" "$REPORT" 0); code=$?
if [[ $code -ne 0 ]] || [[ "$out" != *"push origin pkg@1.0.1"* ]]; then
  echo "FAIL: release-tag-only batch must push the release tag and not error: code=$code out=$out"; fail=1
else
  echo "PASS: release-tag-only batch pushes cleanly with no floating tags"
fi

# Case 4: only a floating alias, no new release tag in this batch --
# must force-push the alias and must not error out attempting to push an
# empty immutable-tags list.
REPORT='{"schemaVersion":1,"tags":[{"package":"pkg","tagName":"pkg@2","sha":"a","alreadyExisted":true,"isFloatingMajor":true}]}'
out=$(run_case "$PLAN" "$REPORT" 0); code=$?
if [[ $code -ne 0 ]] || [[ "$out" != *"push origin --force pkg@2"* ]]; then
  echo "FAIL: floating-alias-only batch must force-push the alias and not error: code=$code out=$out"; fail=1
else
  echo "PASS: floating-alias-only batch pushes cleanly with no immutable tags"
fi

exit $fail
