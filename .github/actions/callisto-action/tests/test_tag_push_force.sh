#!/usr/bin/env bash
# Regression test for a real release run (PR #16, 2026-08-27): a plain
# `git push origin --tags` can never move an already-existing remote tag,
# so `callisto tag`'s floating major-version alias (e.g. `pkg@0`, force-moved
# LOCALLY via `git tag -f`) silently failed to reach the remote on every
# release after the first. The trailing `|| true` hid the failure entirely --
# the job reported success while the alias stayed stale.
#
# This proves the push step forces the tag update and no longer swallows a
# genuine failure.
set -u
ACTION_YML="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/action.yml"

extract_snippet() {
  sed -n '/::group::4\. Tagging Release Commits/,/::endgroup::/p' "$ACTION_YML"
}

fail=0

# The push must be forced, or an already-existing floating tag can never move.
if ! extract_snippet | grep -qE 'git push origin --tags --force'; then
  echo "FAIL: tag push step must force-push tags (git push origin --tags --force)"
  fail=1
else
  echo "PASS: tag push step forces the tag update"
fi

# The push's exit code must not be silently swallowed -- a real failure
# (auth, network, an actual conflicting tag) must fail the job, not hide.
if extract_snippet | grep -E 'git push origin --tags' | grep -q '|| true'; then
  echo "FAIL: tag push step must not swallow its own failure with '|| true'"
  fail=1
else
  echo "PASS: tag push step does not swallow its own failure"
fi

exit $fail
