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
# exact `jq` expressions this action's script uses against its real output,
# for both `release-pr decide` and `release-pr commit-plan`.
set -euo pipefail

action_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workspace_root="$(cd "$action_dir/../../.." && pwd)"

cargo build -p callisto-cli --quiet --manifest-path "$workspace_root/Cargo.toml"
callisto_bin="$workspace_root/target/debug/callisto"

fail=0

# A self-contained workspace with one pending changeset, so decide/verify
# always exercise the update/create paths regardless of this checkout's own
# .changeset/ state.
build_temp_repo() {
  local dir
  dir=$(mktemp -d)
  git -C "$dir" init -q -b main
  git -C "$dir" config user.name 'Contract Test'
  git -C "$dir" config user.email 'contract-test@callisto.dev'
  git -C "$dir" config commit.gpgsign false
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/core"]
resolver = "2"
EOF
  mkdir -p "$dir/crates/core/src"
  cat > "$dir/crates/core/Cargo.toml" <<'EOF'
[package]
name = "core-crate"
version = "0.1.0"
edition = "2021"
EOF
  echo 'pub fn hello() {}' > "$dir/crates/core/src/lib.rs"
  mkdir -p "$dir/.changeset"
  cat > "$dir/.changeset/test-change.md" <<'EOF'
---
core-crate: patch
---

Test change for binary contract coverage.
EOF
  git -C "$dir" add -A
  git -C "$dir" commit -q -m 'initial commit'
  printf '%s' "$dir"
}

assert_jq_matches_real_output() {
  local label="$1" repo="$2" base_commit="$3" expect_kind="$4"
  local snapshot decision decision_kind existing_pr staging_branch

  snapshot=$(jq -cn \
    --arg repository 'orin-dx/callisto' \
    --arg base_branch main \
    --arg base_commit "$base_commit" \
    --arg head_commit "$base_commit" \
    '{schemaVersion: 2, repository: $repository, baseBranch: $base_branch, baseCommit: $base_commit,
      openPullRequests: [{number: 42, headRepository: $repository, headBranch: "callisto/version-packages",
      headCommit: $head_commit}]}')

  decision=$("$callisto_bin" --format json release-pr decide \
    --snapshot "$snapshot" --repository orin-dx/callisto --base-branch main \
    --release-branch callisto/version-packages --cwd "$repo")

  # These are the exact expressions create-or-update-release-pr.sh evaluates.
  decision_kind=$(jq -r '.action.kind' <<< "$decision")
  existing_pr=$(jq -r '.action.pullRequestNumber' <<< "$decision")
  staging_branch=$(jq -r '.action.stagingBranch' <<< "$decision")

  if [[ "$decision_kind" != "$expect_kind" ]]; then
    echo "FAIL ($label): expected decision kind '$expect_kind', got '$decision_kind': $decision"
    fail=1
    return
  fi
  if [[ "$decision_kind" == update && "$existing_pr" != "42" ]]; then
    echo "FAIL ($label): jq -r '.action.pullRequestNumber' resolved to '$existing_pr' instead of 42 -- this is exactly what made the script run \`gh pr view null\`: $decision"
    fail=1
    return
  fi
  if [[ "$staging_branch" != "callisto/version-packages--staging" ]]; then
    echo "FAIL ($label): jq -r '.action.stagingBranch' resolved to '$staging_branch', not the expected deterministic staging branch: $decision"
    fail=1
    return
  fi
  if [[ "$decision_kind" == update ]]; then
    local expected_head
    expected_head=$(jq -r '.action.expectedHeadCommit' <<< "$decision")
    if [[ "$expected_head" != "$base_commit" ]]; then
      echo "FAIL ($label): jq -r '.action.expectedHeadCommit' resolved to '$expected_head', not the PR's real head commit: $decision"
      fail=1
      return
    fi
  fi
  echo "PASS ($label): real binary output round-trips through the script's jq expressions"
}

repo=$(build_temp_repo)
base_commit=$(git -C "$repo" rev-parse HEAD)

assert_jq_matches_real_output 'update (existing managed PR)' "$repo" "$base_commit" update

rm -rf "$repo"
repo=$(build_temp_repo)
base_commit=$(git -C "$repo" rev-parse HEAD)
# No open PR observed at all -> create.
snapshot=$(jq -cn --arg repository 'orin-dx/callisto' --arg base_branch main --arg base_commit "$base_commit" \
  '{schemaVersion: 2, repository: $repository, baseBranch: $base_branch, baseCommit: $base_commit, openPullRequests: []}')
decision=$("$callisto_bin" --format json release-pr decide --snapshot "$snapshot" --repository orin-dx/callisto \
  --base-branch main --release-branch callisto/version-packages --cwd "$repo")
decision_kind=$(jq -r '.action.kind' <<< "$decision")
staging_branch=$(jq -r '.action.stagingBranch' <<< "$decision")
if [[ "$decision_kind" != create || "$staging_branch" != "callisto/version-packages--staging" ]]; then
  echo "FAIL (create, no existing PR): expected kind=create with the deterministic staging branch, got: $decision"
  fail=1
else
  echo 'PASS (create, no existing PR): real binary output round-trips through the script'"'"'s jq expressions'
fi

# release-pr commit-plan: stage a known change and verify the exact jq
# mapping create-or-update-release-pr.sh applies to build the GraphQL
# createCommitOnBranch body, including base64 content, against real output.
printf '1.2.3' > "$repo/VERSION"
git -C "$repo" add -A
plan=$("$callisto_bin" --format json release-pr commit-plan --base-commit "$base_commit" \
  --message 'chore(release): version packages' --cwd "$repo")

body=$(jq -n --arg repo 'orin-dx/callisto' --arg branch 'callisto/version-packages--staging' \
  --arg headline 'chore(release): version packages' --argjson plan "$plan" \
  '{
    query: "mutation($input: CreateCommitOnBranchInput!) { createCommitOnBranch(input: $input) { commit { oid } } }",
    variables: {
      input: {
        branch: {repositoryNameWithOwner: $repo, branchName: $branch},
        message: {headline: $headline},
        expectedHeadOid: $plan.baseCommit,
        fileChanges: {
          additions: [$plan.additions[] | {path, contents: .contentsBase64}],
          deletions: $plan.deletions
        }
      }
    }
  }')

version_addition=$(jq -c '.variables.input.fileChanges.additions[] | select(.path == "VERSION")' <<< "$body")
if [[ -z "$version_addition" ]]; then
  echo "FAIL (commit-plan graphql mapping): no VERSION addition in the mapped body: $body"
  fail=1
else
  decoded=$(jq -r '.contents' <<< "$version_addition" | base64 --decode)
  expected_head_oid=$(jq -r '.variables.input.expectedHeadOid' <<< "$body")
  if [[ "$decoded" != $'1.2.3' ]]; then
    echo "FAIL (commit-plan graphql mapping): VERSION content decoded to '$decoded', expected '1.2.3': $body"
    fail=1
  elif [[ "$expected_head_oid" != "$base_commit" ]]; then
    echo "FAIL (commit-plan graphql mapping): expectedHeadOid was '$expected_head_oid', expected the base commit: $body"
    fail=1
  else
    echo 'PASS (commit-plan graphql mapping): real commit-plan output maps correctly through the script'"'"'s exact jq expression'
  fi
fi

rm -rf "$repo"

exit "$fail"
