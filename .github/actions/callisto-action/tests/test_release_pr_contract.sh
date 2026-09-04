#!/usr/bin/env bash
# Black-box coverage for the exact release-PR action executable. The fake
# process boundary proves the script executes only Callisto's closed decision
# set and only the forge commit API (never `git push`); Rust tests cover
# decision policy itself.
#
# This stubs `callisto` with hand-written JSON fixtures, so it cannot catch a
# mismatch between what the real binary serializes and what this script's jq
# expressions expect -- see test_release_pr_decide_binary_contract.sh, which
# calls the compiled binary, for that check.
set -euo pipefail

action_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="$action_dir/scripts/create-or-update-release-pr.sh"

run_case() {
  local prs="$1" decision="$2" label_exists="${3:-true}"
  local calls_file output_file harness verify_count_file
  calls_file="$(mktemp)"
  output_file="$(mktemp)"
  verify_count_file="$(mktemp)"
  harness="$(mktemp)"
  rm -f "$verify_count_file"

  {
    cat <<'STUBS'
callisto() {
  echo "callisto $*" >> "$CALLS_FILE"
  if [[ " $* " == *' release-pr decide '* ]]; then printf '%s\n' "$DECISION"; return 0; fi
  if [[ " $* " == *' release-pr verify '* ]]; then
    local n
    n=$(( $(cat "$VERIFY_COUNT_FILE" 2>/dev/null || echo 0) + 1 ))
    echo "$n" > "$VERIFY_COUNT_FILE"
    if [[ "$VERIFY_FAIL_AT" != 0 && "$n" == "$VERIFY_FAIL_AT" ]]; then return 1; fi
    return 0
  fi
  if [[ " $* " == *' release-pr commit-plan '* ]]; then
    local out='' prev=''
    for a in "$@"; do
      [[ "$prev" == '--out' ]] && out="$a"
      prev="$a"
    done
    printf '%s\n' "$COMMIT_PLAN" > "$out"
    return "$COMMIT_PLAN_CODE"
  fi
  case "$1" in
    matrix) printf '%s\n' '{"platformTargets":[]}' ;;
    compose-pr-body) echo 'composed release body' ;;
    version) return 0 ;;
    *) echo "unexpected callisto invocation: $*" >&2; return 97 ;;
  esac
}
gh() {
  echo "gh $*" >> "$CALLS_FILE"
  local joined=" $* "
  case "$1 $2" in
    'pr list') printf '%s\n' "$PR_LIST"; return 0 ;;
    'pr view') echo 'existing body'; return 0 ;;
    'pr create') echo "$PR_CREATE_URL"; return 0 ;;
    'pr edit') return 0 ;;
    'label list') echo "$LABEL_EXISTS"; return 0 ;;
    'label create') return 0 ;;
  esac
  if [[ "$1" == 'api' ]]; then
    if [[ "$joined" == *' graphql '* ]]; then
      [[ "$GRAPHQL_CODE" == 0 ]] || return "$GRAPHQL_CODE"
      printf '%s\n' "$NEW_SHA"
      return 0
    fi
    if [[ "$joined" == *'/git/commits/'* ]]; then
      printf '%s\n' "$REMOTE_TREE"
      return 0
    fi
    # The bare collection endpoint (`.../git/refs`, no ref name in the URL
    # path -- the branch only appears in the `-f ref=refs/heads/...` value)
    # is used for ref *creation*: the staging-ref POST fallback and the
    # create-decision's initial attempt to create the real branch.
    if [[ "$joined" == *' -X POST '* && "$joined" == *'/git/refs '* ]]; then
      [[ "$joined" == *'ref=refs/heads/'*'--staging'* ]] && return 0
      return "$MOVE_POST_CODE"
    fi
    # `.../git/refs/heads/<branch>` (the branch name in the URL path itself)
    # is used for PATCH (force-update an existing ref), GET (confirm), and
    # DELETE (staging cleanup).
    if [[ "$joined" == *'/git/refs/heads/'* ]]; then
      if [[ "$joined" == *'/git/refs/heads/'*'--staging'* ]]; then
        [[ "$joined" == *' -X PATCH '* ]] && return "$STAGING_PATCH_CODE"
        return 0
      fi
      [[ "$joined" == *' -X PATCH '* ]] && return "$MOVE_PATCH_CODE"
      printf '%s\n' "$NEW_SHA"
      return 0
    fi
  fi
  return 0
}
git() {
  echo "git $*" >> "$CALLS_FILE"
  case "$1" in
    rev-parse) echo "$BASE_SHA"; return 0 ;;
    add) return 0 ;;
    status) echo ' M Cargo.toml'; return 0 ;;
    write-tree) echo "$LOCAL_TREE"; return 0 ;;
  esac
  return 0
}
STUBS
  } > "$harness"

  set +e
  CALLS_FILE="$calls_file" PR_LIST="$prs" DECISION="$decision" LABEL_EXISTS="$label_exists" \
    VERIFY_FAIL_AT="${VERIFY_FAIL_AT:-0}" VERIFY_COUNT_FILE="$verify_count_file" \
    STAGING_PATCH_CODE="${STAGING_PATCH_CODE:-0}" GRAPHQL_CODE="${GRAPHQL_CODE:-0}" \
    MOVE_PATCH_CODE="${MOVE_PATCH_CODE:-0}" MOVE_POST_CODE="${MOVE_POST_CODE:-0}" \
    COMMIT_PLAN_CODE="${COMMIT_PLAN_CODE:-0}" COMMIT_PLAN="${COMMIT_PLAN:-$default_commit_plan}" \
    NEW_SHA="${NEW_SHA:-$default_new_sha}" LOCAL_TREE="${LOCAL_TREE:-tree-aaaa}" REMOTE_TREE="${REMOTE_TREE:-tree-aaaa}" \
    PR_CREATE_URL="${PR_CREATE_URL:-https://github.com/orin-dx/callisto/pull/99}" \
    BASE_SHA='0123456789abcdef0123456789abcdef01234567' GITHUB_OUTPUT="$output_file" \
    INPUT_VERSION_COMMAND='callisto version' \
    INPUT_COMMIT_MESSAGE='chore(release): version packages' \
    INPUT_TITLE='chore(release): version packages' \
    INPUT_PR_LABEL='callisto: release' \
    INPUT_SETUP_GIT_USER=true \
    INPUT_BRANCH=main \
    INPUT_RELEASE_BRANCH='callisto/version-packages' \
    INPUT_CWD=. \
    GITHUB_REPOSITORY='orin-dx/callisto' \
    bash -c 'source "$1"; source "$2"' -- "$harness" "$script" > /dev/null 2>&1
  local code=$?
  set -e

  printf '%s\n---calls---\n' "$code"
  cat "$calls_file"
  rm -f "$calls_file" "$output_file" "$harness" "$verify_count_file"
}

default_new_sha='deadbeefdeadbeefdeadbeefdeadbeefdeadbeef'
default_commit_plan='{"schemaVersion":1,"baseCommit":"0123456789abcdef0123456789abcdef01234567","message":"chore(release): version packages","additions":[{"path":"VERSION","contentsBase64":"MS4yLjM="}],"deletions":[],"totalContentBytes":5}'

create='{"schemaVersion":2,"action":{"kind":"create","branch":"callisto/version-packages","stagingBranch":"callisto/version-packages--staging"}}'
update='{"schemaVersion":2,"action":{"kind":"update","pullRequestNumber":42,"branch":"callisto/version-packages","expectedHeadCommit":"0123456789abcdef0123456789abcdef01234567","stagingBranch":"callisto/version-packages--staging"}}'
fallback_branch='callisto/version-packages--0123456789abcdef0123456789abcdef01234567'
retained="{\"schemaVersion\":2,\"action\":{\"kind\":\"update\",\"pullRequestNumber\":99,\"branch\":\"$fallback_branch\",\"expectedHeadCommit\":\"0123456789abcdef0123456789abcdef01234567\",\"stagingBranch\":\"callisto/version-packages--staging\"}}"
noop='{"schemaVersion":2,"action":{"kind":"noop","reason":{"kind":"noPendingChangesets"}}}'

fail=0

output="$(run_case '[]' "$create")"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'callisto --format json release-pr decide'* ]] \
  || [[ "$output" != *'callisto --format json release-pr commit-plan'* ]] \
  || [[ "$(grep -F -c 'callisto release-pr verify' <<< "$output")" != 3 ]] \
  || [[ "$output" != *"gh api -X PATCH repos/orin-dx/callisto/git/refs/heads/callisto/version-packages--staging -f sha=0123456789abcdef0123456789abcdef01234567 -F force=true"* ]] \
  || [[ "$output" != *'gh api graphql'* ]] \
  || [[ "$output" != *"gh api -X POST repos/orin-dx/callisto/git/refs -f ref=refs/heads/callisto/version-packages -f sha=$default_new_sha"* ]] \
  || [[ "$output" != *"gh api -X DELETE repos/orin-dx/callisto/git/refs/heads/callisto/version-packages--staging"* ]] \
  || [[ "$output" != *'gh pr create --head callisto/version-packages --base main'* ]] \
  || [[ "$output" == *'git push'* ]]; then
  echo "FAIL: executes Callisto create decision through the forge commit API with three mutation-boundary checks: $output"
  fail=1
else
  echo 'PASS: executes Callisto create decision'
fi

output="$(run_case '[{"number":42,"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"orin-dx/callisto"},"headRefOid":"0123456789abcdef0123456789abcdef01234567"}]' "$update" false)"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'gh label create callisto: release'* ]] \
  || [[ "$output" != *"gh api -X PATCH repos/orin-dx/callisto/git/refs/heads/callisto/version-packages -f sha=$default_new_sha -F force=true"* ]] \
  || [[ "$output" != *'gh pr edit 42'* ]] \
  || [[ "$output" == *'gh pr create'* ]] \
  || [[ "$output" == *'git push'* ]]; then
  echo "FAIL: executes Callisto update decision through the forge commit API: $output"
  fail=1
else
  echo 'PASS: executes Callisto update decision'
fi

output="$(run_case "[{\"number\":99,\"headRefName\":\"$fallback_branch\",\"headRepository\":{\"nameWithOwner\":\"orin-dx/callisto\"},\"headRefOid\":\"0123456789abcdef0123456789abcdef01234567\"}]" "$retained")"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *"gh api -X PATCH repos/orin-dx/callisto/git/refs/heads/$fallback_branch"* ]] \
  || [[ "$output" != *'gh pr edit 99'* ]] \
  || [[ "$output" == *'gh pr create'* ]] \
  || [[ "$output" == *'git push'* ]]; then
  echo "FAIL: retains a Callisto-selected SHA-suffixed replacement branch and updates it in place: $output"
  fail=1
else
  echo 'PASS: retains and updates a SHA-suffixed replacement branch'
fi

output="$(run_case '[]' "$noop")"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" == *'git push'* ]] \
  || [[ "$output" == *'gh api'* ]] \
  || [[ "$output" == *'gh pr create'* ]] \
  || [[ "$output" == *'gh pr edit'* ]]; then
  echo "FAIL: no-op decision must stop before mutation: $output"
  fail=1
else
  echo 'PASS: no-op decision stops before mutation'
fi

VERIFY_FAIL_AT=1
output="$(run_case '[]' "$create")"
unset VERIFY_FAIL_AT
if [[ "$output" != 1$'\n'* ]] \
  || [[ "$output" == *'gh api'* ]] \
  || [[ "$output" == *'gh label '* ]] \
  || [[ "$output" == *'gh pr create'* ]] \
  || [[ "$output" == *'gh pr edit'* ]]; then
  echo "FAIL: a stale snapshot before staging must prevent every forge mutation: $output"
  fail=1
else
  echo 'PASS: stale snapshot before staging prevents forge mutation'
fi

VERIFY_FAIL_AT=2
output="$(run_case '[]' "$create")"
unset VERIFY_FAIL_AT
if [[ "$output" != 1$'\n'* ]] \
  || [[ "$output" != *'gh api -X PATCH repos/orin-dx/callisto/git/refs/heads/callisto/version-packages--staging'* ]] \
  || [[ "$output" != *'gh api graphql'* ]] \
  || [[ "$output" == *"refs/heads/callisto/version-packages -f sha=$default_new_sha"* ]] \
  || [[ "$output" != *"gh api -X DELETE repos/orin-dx/callisto/git/refs/heads/callisto/version-packages--staging"* ]] \
  || [[ "$output" == *'gh pr create'* ]] \
  || [[ "$output" == *'gh pr edit'* ]]; then
  echo "FAIL: a moved PR head caught before the ref move must leave the real branch untouched and still clean up staging: $output"
  fail=1
else
  echo 'PASS: a moved head before the ref move leaves the real branch untouched and cleans up staging'
fi

REMOTE_TREE='tree-bbbb'
output="$(run_case '[]' "$create")"
unset REMOTE_TREE
if [[ "$output" != 1$'\n'* ]] \
  || [[ "$output" == *"refs/heads/callisto/version-packages -f sha=$default_new_sha"* ]] \
  || [[ "$output" != *"gh api -X DELETE repos/orin-dx/callisto/git/refs/heads/callisto/version-packages--staging"* ]] \
  || [[ "$output" == *'gh pr create'* ]]; then
  echo "FAIL: a tree mismatch between the forge commit and the local staged tree must refuse to move the managed branch: $output"
  fail=1
else
  echo 'PASS: a tree mismatch refuses to move the managed branch and still cleans up staging'
fi

STAGING_PATCH_CODE=1
output="$(run_case '[]' "$create")"
unset STAGING_PATCH_CODE
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'gh api -X PATCH repos/orin-dx/callisto/git/refs/heads/callisto/version-packages--staging'* ]] \
  || [[ "$output" != *'gh api -X POST repos/orin-dx/callisto/git/refs -f ref=refs/heads/callisto/version-packages--staging -f sha=0123456789abcdef0123456789abcdef01234567'* ]]; then
  echo "FAIL: an orphaned staging ref from a crashed prior run must fall back from PATCH to POST: $output"
  fail=1
else
  echo 'PASS: staging ref creation falls back from PATCH to POST for an orphaned ref'
fi

MOVE_PATCH_CODE=1
MOVE_POST_CODE=1
output="$(run_case '[]' "$create")"
unset MOVE_PATCH_CODE MOVE_POST_CODE
if [[ "$output" != 1$'\n'* ]] \
  || [[ "$output" != *"gh api -X DELETE repos/orin-dx/callisto/git/refs/heads/callisto/version-packages--staging"* ]] \
  || [[ "$output" == *'gh pr create'* ]]; then
  echo "FAIL: cleanup must run even when moving the managed branch itself fails: $output"
  fail=1
else
  echo 'PASS: staging is cleaned up even when moving the managed branch fails'
fi

action_contents="$(<"$action_dir/action.yml")"
if [[ "$action_contents" != *'bash "$GITHUB_ACTION_PATH/scripts/create-or-update-release-pr.sh"'* ]]; then
  echo 'FAIL: action metadata does not invoke the tested implementation script'
  fail=1
else
  echo 'PASS: action metadata invokes the tested implementation script'
fi

script_contents="$(<"$script")"
if [[ "$script_contents" == *'git push'* ]]; then
  echo 'FAIL: the executor must never git push -- GITHUB_TOKEN cannot write .github/workflows/* through that protocol on a public repository'
  fail=1
elif [[ "$script_contents" != *'callisto --format json release-pr decide'* ]] \
  || [[ "$script_contents" != *'callisto release-pr verify'* ]] \
  || [[ "$script_contents" != *'callisto --format json release-pr commit-plan'* ]]; then
  echo 'FAIL: action still derives release-PR policy outside Callisto'
  fail=1
else
  echo 'PASS: action delegates policy to Callisto and never git-pushes'
fi

# The configured branch is policy input to Callisto, not an action-side branch
# matching rule. A second reference would be a strong signal that the adapter
# has started to recreate release-PR policy.
release_branch_reference_count=$(grep -o 'INPUT_RELEASE_BRANCH' "$script" | wc -l | tr -d ' ')
if [[ "$release_branch_reference_count" != 1 ]]; then
  echo 'FAIL: action must pass the configured release branch to Callisto exactly once'
  fail=1
else
  echo 'PASS: action does not recreate managed-branch policy'
fi

exit "$fail"
