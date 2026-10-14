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
  if ! verify_output=$("$callisto_bin" release-pr verify --decision "$decision" --snapshot "$snapshot" --cwd "$repo" 2>&1); then
    echo "FAIL ($label): release-pr verify rejected release-pr decide's own output: $verify_output"
    fail=1
    return
  fi
  echo "PASS ($label): real binary output round-trips through the script's jq expressions and verify"
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
elif ! verify_output=$("$callisto_bin" release-pr verify --decision "$decision" --snapshot "$snapshot" --cwd "$repo" 2>&1); then
  echo "FAIL (create, no existing PR): release-pr verify rejected release-pr decide's own output: $verify_output"
  fail=1
else
  echo 'PASS (create, no existing PR): real binary output round-trips through the script'"'"'s jq expressions'
fi

assert_verify_transport() {
  local label="$1" decision_json="$2" snapshot_json="$3" transport="$4"
  local applied_head="${5:-}"
  local -a verify_args=(--format json release-pr verify --snapshot "$snapshot_json" --cwd "$repo")
  [[ -z "$applied_head" ]] || verify_args+=(--applied-head "$applied_head")
  local decision_arg verify_output
  case "$transport" in
    inline) decision_arg="$decision_json" ;;
    file)
      decision_arg="$repo/decision.json"
      jq . <<< "$decision_json" > "$decision_arg"
      ;;
    stdin) decision_arg=- ;;
    *) echo "FAIL ($label): unknown transport '$transport'"; fail=1; return ;;
  esac
  if [[ "$transport" == stdin ]]; then
    verify_output=$("$callisto_bin" "${verify_args[@]}" --decision - <<< "$decision_json") || {
      echo "FAIL ($label): verify rejected unchanged decide output: $verify_output"
      fail=1
      return
    }
  else
    verify_output=$("$callisto_bin" "${verify_args[@]}" --decision "$decision_arg") || {
      echo "FAIL ($label): verify rejected unchanged decide output: $verify_output"
      fail=1
      return
    }
  fi
  if [[ "$(jq -r '.ok' <<< "$verify_output")" != true ]]; then
    echo "FAIL ($label): verify did not report success: $verify_output"
    fail=1
  else
    echo "PASS ($label): decide output verifies unchanged"
  fi
}

update_snapshot=$(jq -cn --arg repository 'orin-dx/callisto' --arg base_branch main --arg base_commit "$base_commit" \
  --arg head_commit "$base_commit" \
  '{schemaVersion: 2, repository: $repository, baseBranch: $base_branch, baseCommit: $base_commit,
    openPullRequests: [{number: 42, headRepository: $repository, headBranch: "callisto/version-packages",
    headCommit: $head_commit}]}')
create_snapshot=$(jq -cn --arg repository 'orin-dx/callisto' --arg base_branch main --arg base_commit "$base_commit" \
  '{schemaVersion: 2, repository: $repository, baseBranch: $base_branch, baseCommit: $base_commit, openPullRequests: []}')
update_decision=$("$callisto_bin" --format json release-pr decide --snapshot "$update_snapshot" \
  --repository orin-dx/callisto --base-branch main --release-branch callisto/version-packages --cwd "$repo")
create_decision=$("$callisto_bin" --format json release-pr decide --snapshot "$create_snapshot" \
  --repository orin-dx/callisto --base-branch main --release-branch callisto/version-packages --cwd "$repo")
for transport in inline file stdin; do
  assert_verify_transport "update via $transport" "$update_decision" "$update_snapshot" "$transport"
  assert_verify_transport "create via $transport" "$create_decision" "$create_snapshot" "$transport"
done

# A workspace with no pending changesets produces the no-op decision.
rm "$repo/.changeset/test-change.md"
noop_decision=$("$callisto_bin" --format json release-pr decide --snapshot "$create_snapshot" \
  --repository orin-dx/callisto --base-branch main --release-branch callisto/version-packages --cwd "$repo")
if [[ "$(jq -r '.action.kind' <<< "$noop_decision")" != noop ]]; then
  echo "FAIL (no-op decision): expected kind=noop, got: $noop_decision"
  fail=1
else
  for transport in inline file stdin; do
    assert_verify_transport "no-op via $transport" "$noop_decision" "$create_snapshot" "$transport"
  done
fi

# Verify also accepts the durable decision shape without report metadata.
unwrapped_decision=$(jq -c 'del(.command, .dryRun)' <<< "$update_decision")
for transport in inline file stdin; do
  assert_verify_transport "unwrapped decision via $transport" "$unwrapped_decision" "$update_snapshot" "$transport"
done

assert_verify_rejects() {
  local label="$1" decision_json="$2" snapshot_json="$3" expected_code="$4" output status
  local applied_head="${5:-}"
  local -a verify_args=(--format json release-pr verify --snapshot "$snapshot_json" --cwd "$repo")
  [[ -z "$applied_head" ]] || verify_args+=(--applied-head "$applied_head")
  set +e
  output=$("$callisto_bin" "${verify_args[@]}" --decision "$decision_json" 2>&1)
  status=$?
  set -e
  if [[ "$status" -eq 0 ]]; then
    echo "FAIL ($label): verify unexpectedly accepted input: $output"
    fail=1
  elif [[ -n "$expected_code" && "$(jq -r '.error.code // empty' <<< "$output")" != "$expected_code" ]]; then
    echo "FAIL ($label): expected diagnostic $expected_code, got: $output"
    fail=1
  else
    echo "PASS ($label): verify rejected invalid input"
  fi
}

assert_verify_rejects 'malformed decision JSON' '{not json' "$update_snapshot" E281
assert_verify_rejects 'unsupported decision schema' "$(jq -c '.schemaVersion = 1' <<< "$update_decision")" "$update_snapshot" E155
assert_verify_rejects 'wrong report command' "$(jq -c '.command = "release-pr decide-old"' <<< "$update_decision")" "$update_snapshot" E281
assert_verify_rejects 'null report command' "$(jq -c '.command = null' <<< "$update_decision")" "$update_snapshot" E281
assert_verify_rejects 'null report metadata' "$(jq -c '.dryRun = null' <<< "$update_decision")" "$update_snapshot" E281
assert_verify_rejects 'wrong report metadata type' "$(jq -c '.dryRun = "false"' <<< "$update_decision")" "$update_snapshot" E281
assert_verify_rejects 'partial report metadata' "$(jq -c 'del(.dryRun)' <<< "$update_decision")" "$update_snapshot" E281
assert_verify_rejects 'partial report command' "$(jq -c 'del(.command)' <<< "$update_decision")" "$update_snapshot" E281
assert_verify_rejects 'unknown decision payload field' "$(jq -c '.unexpected = true' <<< "$unwrapped_decision")" "$update_snapshot" E281
assert_verify_rejects 'invalid nested config branch' "$(jq -c '.config.releaseBranch = "bad..branch"' <<< "$unwrapped_decision")" "$update_snapshot" E142
assert_verify_rejects 'invalid nested snapshot schema' "$(jq -c '.snapshot.schemaVersion = 1' <<< "$unwrapped_decision")" "$update_snapshot" E155
stale_snapshot=$(jq -c '.baseCommit = "0000000000000000000000000000000000000000"' <<< "$update_snapshot")
assert_verify_rejects 'stale snapshot' "$update_decision" "$stale_snapshot" E148
wrong_repository_snapshot=$(jq -c '.repository = "other/callisto"' <<< "$update_snapshot")
assert_verify_rejects 'wrong snapshot repository' "$update_decision" "$wrong_repository_snapshot" E148

applied_head=cccccccccccccccccccccccccccccccccccccccc
applied_snapshot=$(jq -c --arg head "$applied_head" '.openPullRequests[0].headCommit = $head' <<< "$update_snapshot")
for transport in inline file stdin; do
  assert_verify_transport "applied update via $transport" "$update_decision" "$applied_snapshot" "$transport" "$applied_head"
done
assert_verify_rejects 'moved head without applied transition' "$update_decision" "$applied_snapshot" E148
assert_verify_rejects 'applied head not observed' "$update_decision" "$update_snapshot" E148 "$applied_head"
assert_verify_rejects 'unrelated base change after update' "$update_decision" \
  "$(jq -c '.baseCommit = "dddddddddddddddddddddddddddddddddddddddd"' <<< "$applied_snapshot")" E148 "$applied_head"
assert_verify_rejects 'create cannot claim an applied update' "$create_decision" "$create_snapshot" E148 "$applied_head"
assert_verify_rejects 'noop cannot claim an applied update' "$noop_decision" "$create_snapshot" E148 "$applied_head"

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
