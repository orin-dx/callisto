#!/usr/bin/env bash
# test_release_pr_contract.sh stubs `callisto` with hand-written camelCase
# JSON fixtures, so it only proves the script's control flow given already
# correctly-shaped input -- it structurally cannot catch a mismatch between
# what the real binary serializes and what this script's `jq` expects. That
# gap is exactly how a snake_case regression in ReleasePrActionV1 (fields
# inside a #[serde(rename_all = "camelCase", tag = "kind")] enum are NOT
# renamed by rename_all -- only the tag is) shipped to production and broke
# every release run past the first (`gh pr view null`, from `jq -r
# '.action.pullRequestNumber'` reading a field that didn't exist).
#
# This test compiles and calls the real `callisto` binary and asserts the
# exact `jq` expressions this action's script uses against its real output.
set -euo pipefail

action_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workspace_root="$(cd "$action_dir/../../.." && pwd)"

cargo build -p callisto-cli --quiet --manifest-path "$workspace_root/Cargo.toml"
callisto_bin="$workspace_root/target/debug/callisto"

fail=0

assert_jq_matches_real_output() {
  local label="$1" workflow_delta="$2" expect_kind="$3"
  local snapshot decision decision_kind existing_pr

  snapshot=$(jq -cn \
    --arg repository 'orin-dx/callisto' \
    --arg base_branch main \
    --arg base_commit '0123456789abcdef0123456789abcdef01234567' \
    --argjson workflow_delta "$workflow_delta" \
    '{schemaVersion: 1, repository: $repository, baseBranch: $base_branch, baseCommit: $base_commit,
      openPullRequests: [{number: 42, headRepository: $repository, headBranch: "callisto/version-packages",
      workflowDeltaFromBase: $workflow_delta}]}')

  decision=$("$callisto_bin" --format json release-pr decide \
    --snapshot "$snapshot" --repository orin-dx/callisto --base-branch main \
    --release-branch callisto/version-packages --cwd "$workspace_root")

  # These are the exact expressions create-or-update-release-pr.sh evaluates.
  decision_kind=$(jq -r '.action.kind' <<< "$decision")
  existing_pr=$(jq -r '.action.pullRequestNumber' <<< "$decision")

  if [[ "$decision_kind" != "$expect_kind" ]]; then
    echo "FAIL ($label): expected decision kind '$expect_kind', got '$decision_kind': $decision"
    fail=1
    return
  fi
  if [[ "$existing_pr" != "42" ]]; then
    echo "FAIL ($label): jq -r '.action.pullRequestNumber' resolved to '$existing_pr' instead of 42 -- this is exactly what made the script run \`gh pr view null\`: $decision"
    fail=1
    return
  fi
  echo "PASS ($label): real binary output round-trips through the script's jq expressions"
}

# has_pending_changesets must be true against this workspace for either case
# below to produce anything but a noop -- true as long as a changeset file
# is pending, which this repository's own release process requires anyway.
shopt -s nullglob
changesets=("$workspace_root"/.changeset/*.md)
shopt -u nullglob
if [[ ${#changesets[@]} -eq 0 ]]; then
  echo 'SKIP: no pending changesets in this checkout; cannot exercise update/supersede decisions'
  exit 0
fi

assert_jq_matches_real_output 'update (no workflow delta)' false update
assert_jq_matches_real_output 'supersede (workflow delta)' true supersede

exit "$fail"
