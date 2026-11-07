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
  local calls_file output_file harness verify_count_file stale_file transient_file
  calls_file="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  output_file="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  verify_count_file="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  harness="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  rm -f "$verify_count_file"
  stale_file="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  : > "$stale_file"
  transient_file="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  rm -f "$transient_file"

  {
    cat <<'STUBS'
callisto() {
  echo "callisto $*" >> "$CALLS_FILE"
  if [[ " $* " == *' release-pr decide '* ]]; then printf '%s\n' "$DECISION"; return 0; fi
  if [[ " $* " == *' release-pr verify '* ]]; then
    local n arg previous='' applied_head='' snapshot=''
    n=$(( $(cat "$VERIFY_COUNT_FILE" 2>/dev/null || echo 0) + 1 ))
    echo "$n" > "$VERIFY_COUNT_FILE"
    if [[ "$VERIFY_FAIL_AT" != 0 && "$n" == "$VERIFY_FAIL_AT" ]]; then return 1; fi
    if [[ "$VERIFY_FAIL_AT" != 0 && "$VERIFY_FAIL_PERSIST" == true && "$n" -gt "$VERIFY_FAIL_AT" ]]; then return 1; fi
    if [[ "$REQUIRE_APPLIED_HEAD" == true && "$n" -ge 3 ]]; then
      for arg in "$@"; do
        case "$previous" in
          --applied-head) applied_head="$arg" ;;
          --snapshot) snapshot="$arg" ;;
        esac
        previous="$arg"
      done
      if [[ "$applied_head" != "$NEW_SHA" ]]; then
        echo 'verification rejected missing or wrong applied head' >> "$CALLS_FILE"
        return 1
      fi
      jq -e --argjson number "$(jq -r '.action.pullRequestNumber' <<< "$DECISION")" --arg head "$NEW_SHA" \
        'any(.openPullRequests[]; .number == $number and .headCommit == $head)' <<< "$snapshot" >/dev/null \
        || return 1
      echo 'verification accepted the applied head observed on the moved PR' >> "$CALLS_FILE"
    fi
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
  if [[ -n "$TRANSIENT_ON" && "$joined" == *"$TRANSIENT_ON"* ]]; then
    local seen=$(( $(cat "$TRANSIENT_FILE" 2>/dev/null || echo 0) + 1 ))
    echo "$seen" > "$TRANSIENT_FILE"
    if [[ "$seen" -gt "$TRANSIENT_SKIP" && "$seen" -le $(( TRANSIENT_SKIP + TRANSIENT_FAILS )) ]]; then
      echo "gh: ${TRANSIENT_MESSAGE:-Something went wrong while executing your query on 2026-11-02T16:57:39Z. Please include \`ID\` when reporting this issue.}" >&2
      return 1
    fi
  fi
  case "$1 $2" in
    'pr list')
      if [[ -n "$PR_LIST_FAIL_AFTER" ]]; then
        local listed=$(( $(cat "$PR_LIST_COUNT_FILE" 2>/dev/null || echo 0) + 1 ))
        echo "$listed" > "$PR_LIST_COUNT_FILE"
        if [[ "$listed" -gt "$PR_LIST_FAIL_AFTER" ]]; then echo 'gh: HTTP 403: forbidden' >&2; return 4; fi
      fi
      if [[ -s "$STALE_FILE" && "$(cat "$STALE_FILE")" -gt 0 ]]; then
        echo $(( $(cat "$STALE_FILE") - 1 )) > "$STALE_FILE"
        printf '%s\n' "$STALE_PR_LIST"
        return 0
      fi
      printf '%s\n' "$PR_LIST"; return 0 ;;
    'pr view')
      if [[ "$joined" == *' isDraft '* ]]; then echo "$IS_DRAFT"; else echo 'existing body'; fi
      return 0 ;;
    'pr ready') return 0 ;;
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
    if [[ "$joined" == *'/git/ref/heads/'* ]]; then
      printf '%s\n' "$BASE_TIP"
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
      if [[ "$joined" == *' -X PATCH '* ]]; then
        [[ "$MOVE_PATCH_CODE" == 0 ]] || return "$MOVE_PATCH_CODE"
        if [[ "$REQUIRE_APPLIED_HEAD" == true ]]; then
          STALE_PR_LIST="$PR_LIST"
          [[ "$STALE_READS" -gt 0 ]] && echo "$STALE_READS" > "$STALE_FILE"
          PR_LIST=$(jq --argjson number "$(jq -r '.action.pullRequestNumber' <<< "$DECISION")" --arg head "$NEW_SHA" \
            'map(if .number == $number then .headRefOid = $head else . end)' <<< "$PR_LIST")
        fi
        return 0
      fi
      printf '%s\n' "$NEW_SHA"
      return 0
    fi
  fi
  return 0
}
sleep() { echo "sleep $*" >> "$CALLS_FILE"; }
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
    VERIFY_FAIL_AT="${VERIFY_FAIL_AT:-0}" VERIFY_FAIL_PERSIST="${VERIFY_FAIL_PERSIST:-false}" VERIFY_COUNT_FILE="$verify_count_file" \
    STALE_READS="${STALE_READS:-0}" STALE_FILE="$stale_file" \
    TRANSIENT_ON="${TRANSIENT_ON:-}" TRANSIENT_FAILS="${TRANSIENT_FAILS:-0}" TRANSIENT_FILE="$transient_file" \
    TRANSIENT_MESSAGE="${TRANSIENT_MESSAGE:-}" TRANSIENT_SKIP="${TRANSIENT_SKIP:-0}" IS_DRAFT="${IS_DRAFT:-false}" \
    PR_LIST_FAIL_AFTER="${PR_LIST_FAIL_AFTER:-}" PR_LIST_COUNT_FILE="$transient_file.prlist" \
    BASE_TIP="${BASE_TIP:-0123456789abcdef0123456789abcdef01234567}" \
    REQUIRE_APPLIED_HEAD="${REQUIRE_APPLIED_HEAD:-false}" \
    STAGING_PATCH_CODE="${STAGING_PATCH_CODE:-0}" GRAPHQL_CODE="${GRAPHQL_CODE:-0}" \
    MOVE_PATCH_CODE="${MOVE_PATCH_CODE:-0}" MOVE_POST_CODE="${MOVE_POST_CODE:-0}" \
    COMMIT_PLAN_CODE="${COMMIT_PLAN_CODE:-0}" COMMIT_PLAN="${COMMIT_PLAN:-$default_commit_plan}" \
    NEW_SHA="${NEW_SHA:-$default_new_sha}" LOCAL_TREE="${LOCAL_TREE:-tree-aaaa}" REMOTE_TREE="${REMOTE_TREE:-tree-aaaa}" \
    PR_CREATE_URL="${PR_CREATE_URL:-https://github.com/orin-dx/callisto/pull/99}" \
    BASE_SHA='0123456789abcdef0123456789abcdef01234567' GITHUB_OUTPUT="$output_file" \
    INPUT_VERSION_COMMAND='callisto version' \
    INPUT_DECISION_PATH='.callisto/release-decision.json' \
    INPUT_COMMIT_MESSAGE='chore(release): version packages' \
    INPUT_TITLE='chore(release): version packages' \
    INPUT_PR_LABEL='callisto: release' \
    INPUT_BRANCH=main \
    INPUT_RELEASE_BRANCH='callisto/version-packages' \
    INPUT_CWD=. \
    GITHUB_REPOSITORY='orin-dx/callisto' \
    bash -c 'source "$1"; source "$2"' -- "$harness" "$script" > /dev/null 2>&1
  local code=$?
  set -e

  printf '%s\n---calls---\n' "$code"
  cat "$calls_file"
  printf '%s\n' '---outputs---'
  cat "$output_file"
  rm -f "$calls_file" "$output_file" "$harness" "$verify_count_file" "$stale_file" "$transient_file" "$transient_file.prlist"
}

default_new_sha='deadbeefdeadbeefdeadbeefdeadbeefdeadbeef'
default_commit_plan='{"schemaVersion":1,"baseCommit":"0123456789abcdef0123456789abcdef01234567","message":"chore(release): version packages","additions":[{"path":"VERSION","contentsBase64":"MS4yLjM="}],"deletions":[],"totalContentBytes":5}'

create='{"schemaVersion":2,"action":{"kind":"create","branch":"callisto/version-packages","stagingBranch":"callisto/version-packages--staging"}}'
update='{"schemaVersion":2,"action":{"kind":"update","pullRequestNumber":42,"branch":"callisto/version-packages","expectedHeadCommit":"0123456789abcdef0123456789abcdef01234567","stagingBranch":"callisto/version-packages--staging"}}'
fallback_branch='callisto/version-packages--0123456789abcdef0123456789abcdef01234567'
retained="{\"schemaVersion\":2,\"action\":{\"kind\":\"update\",\"pullRequestNumber\":99,\"branch\":\"$fallback_branch\",\"expectedHeadCommit\":\"0123456789abcdef0123456789abcdef01234567\",\"stagingBranch\":\"callisto/version-packages--staging\"}}"
noop='{"schemaVersion":2,"action":{"kind":"noop","reason":{"kind":"noPendingChangesets"}}}'

fail=0

assert_verify_flags() {
  local label="$1" output="$2" applied_head="$3" line
  local -a verifies=()
  while IFS= read -r line; do
    if [[ "$line" == 'callisto release-pr verify '* ]]; then
      verifies+=("$line")
    fi
  done <<< "$output"
  if [[ ${#verifies[@]} -ne 3 ]] \
    || [[ "${verifies[0]}" == *' --applied-head '* ]] \
    || [[ "${verifies[1]}" == *' --applied-head '* ]]; then
    echo "FAIL: $label must keep the first two verification calls strict: $output"
    fail=1
  elif [[ -n "$applied_head" && "${verifies[2]}" != *" --applied-head $applied_head" ]] \
    || [[ -z "$applied_head" && "${verifies[2]}" == *' --applied-head '* ]]; then
    echo "FAIL: $label final verification used the wrong applied head: $output"
    fail=1
  else
    echo "PASS: $label verification flags"
  fi
}

output="$(run_case '[]' "$create")"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'callisto --format json release-pr decide'* ]] \
  || [[ "$output" != *'callisto --format json release-pr commit-plan'* ]] \
  || [[ "$(grep -F -c 'callisto release-pr verify' <<< "$output")" != 3 ]] \
  || [[ "$output" != *"gh api -X PATCH repos/orin-dx/callisto/git/refs/heads/callisto/version-packages--staging -f sha=0123456789abcdef0123456789abcdef01234567 -F force=true"* ]] \
  || [[ "$output" != *'gh api graphql'* ]] \
  || [[ "$output" != *"gh api -X POST repos/orin-dx/callisto/git/refs -f ref=refs/heads/callisto/version-packages -f sha=$default_new_sha"* ]] \
  || [[ "$output" != *"gh api -X DELETE repos/orin-dx/callisto/git/refs/heads/callisto/version-packages--staging"* ]] \
  || [[ "$output" != *'gh pr create --draft --head callisto/version-packages --base main'* ]] \
  || [[ "$output" != *'--label callisto: release'* ]] \
  || [[ "$output" == *'git push'* ]] \
  || [[ "$output" == *'nativeMatrix'* ]]; then
  echo "FAIL: executes Callisto create decision through the forge commit API with three mutation-boundary checks: $output"
  fail=1
else
  echo 'PASS: executes Callisto create decision'
fi
assert_verify_flags create "$output" ''

if [[ "$output" != *'--emit-preview '*'callisto compose-pr-body --preview '* ]]; then
  echo "FAIL: the version command writes the preview before compose-pr-body reads it: $output"
  fail=1
else
  echo 'PASS: composes the body from the preview the version command wrote'
fi

output="$(run_case '[{"number":42,"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"orin-dx/callisto"},"headRefOid":"0123456789abcdef0123456789abcdef01234567"}]' "$update" false)"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'gh label create callisto: release'* ]] \
  || [[ "$output" != *"gh api -X PATCH repos/orin-dx/callisto/git/refs/heads/callisto/version-packages -f sha=$default_new_sha -F force=true"* ]] \
  || [[ "$output" != *'gh pr edit 42'* ]] \
  || [[ "$output" != *'--add-label callisto: release'* ]] \
  || [[ "$output" == *'gh pr create'* ]] \
  || [[ "$output" == *'git push'* ]] \
  || [[ "$output" == *'nativeMatrix'* ]]; then
  echo "FAIL: executes Callisto update decision through the forge commit API: $output"
  fail=1
else
  echo 'PASS: executes Callisto update decision'
fi
assert_verify_flags update "$output" "$default_new_sha"

output="$(run_case "[{\"number\":99,\"headRefName\":\"$fallback_branch\",\"headRepository\":{\"nameWithOwner\":\"orin-dx/callisto\"},\"headRefOid\":\"0123456789abcdef0123456789abcdef01234567\"}]" "$retained")"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *"gh api -X PATCH repos/orin-dx/callisto/git/refs/heads/$fallback_branch"* ]] \
  || [[ "$output" != *'gh pr edit 99'* ]] \
  || [[ "$output" == *'gh pr create'* ]] \
  || [[ "$output" == *'git push'* ]] \
  || [[ "$output" == *'nativeMatrix'* ]]; then
  echo "FAIL: retains a Callisto-selected SHA-suffixed replacement branch and updates it in place: $output"
  fail=1
else
  echo 'PASS: retains and updates a SHA-suffixed replacement branch'
fi
assert_verify_flags 'retained update' "$output" "$default_new_sha"

NEW_SHA='abcdefabcdefabcdefabcdefabcdefabcdefabcd'
REQUIRE_APPLIED_HEAD=true
output="$(run_case '[{"number":42,"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"orin-dx/callisto"},"headRefOid":"0123456789abcdef0123456789abcdef01234567"}]' "$update")"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'verification accepted the applied head observed on the moved PR'* ]] \
  || [[ "$output" != *'gh pr edit 42'* ]]; then
  echo "FAIL: update must verify the applied head after the forge moves the PR head: $output"
  fail=1
else
  echo 'PASS: update verifies the applied head after the forge moves the PR head'
fi
assert_verify_flags 'update with another commit' "$output" "$NEW_SHA"
unset NEW_SHA REQUIRE_APPLIED_HEAD

# The PR list can lag the ref move; a stale read must be retried, not rejected.
NEW_SHA='abcdefabcdefabcdefabcdefabcdefabcdefabcd'
REQUIRE_APPLIED_HEAD=true
STALE_READS=2
output="$(run_case '[{"number":42,"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"orin-dx/callisto"},"headRefOid":"0123456789abcdef0123456789abcdef01234567"}]' "$update")"
unset STALE_READS
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'sleep '* ]] \
  || [[ "$output" != *'verification accepted the applied head observed on the moved PR'* ]] \
  || [[ "$output" != *'gh pr edit 42'* ]]; then
  echo "FAIL: a stale PR list right after the ref move must be retried until it shows the applied head: $output"
  fail=1
else
  echo 'PASS: update retries a stale PR list after the ref move'
fi

# A head that never shows the applied commit is a real change and still fails.
STALE_READS=1000
output="$(run_case '[{"number":42,"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"orin-dx/callisto"},"headRefOid":"0123456789abcdef0123456789abcdef01234567"}]' "$update" false)"
unset STALE_READS NEW_SHA REQUIRE_APPLIED_HEAD
if [[ "$output" != 1$'\n'* ]] \
  || [[ "$output" == *'gh pr edit'* ]] \
  || [[ "$output" == *'gh label '* ]]; then
  echo "FAIL: a PR head that never shows the applied commit must fail without label or PR writes: $output"
  fail=1
else
  echo 'PASS: update still fails when the applied head never appears'
fi

# A create or update decision with no branch fails before any write.
nobranch_create='{"schemaVersion":2,"action":{"kind":"create","stagingBranch":"callisto/version-packages--staging"}}'
nobranch_update='{"schemaVersion":2,"action":{"kind":"update","pullRequestNumber":42,"expectedHeadCommit":"0123456789abcdef0123456789abcdef01234567","stagingBranch":"callisto/version-packages--staging"}}'
for missing in "$nobranch_create" "$nobranch_update"; do
  output="$(run_case '[{"number":42,"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"orin-dx/callisto"},"headRefOid":"0123456789abcdef0123456789abcdef01234567"}]' "$missing")"
  if [[ "$output" != 1$'\n'* ]] \
    || [[ "$output" == *'refs/heads/null'* ]] \
    || [[ "$output" == *'gh api'* ]] \
    || [[ "$output" == *'gh pr create'* ]] \
    || [[ "$output" == *'gh pr edit'* ]]; then
    echo "FAIL: a decision with no branch must fail before any write: $output"
    fail=1
  else
    echo 'PASS: a decision with no branch fails before any write'
  fi
done

output="$(run_case '[]' "$noop")"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" == *'git push'* ]] \
  || [[ "$output" == *'gh api'* ]] \
  || [[ "$output" == *'gh pr create'* ]] \
  || [[ "$output" == *'gh pr edit'* ]] \
  || [[ "$output" == *'nativeMatrix'* ]]; then
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

for kind in create update; do
  VERIFY_FAIL_AT=3
  VERIFY_FAIL_PERSIST=true
  if [[ "$kind" == update ]]; then
    output="$(run_case '[{"number":42,"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"orin-dx/callisto"},"headRefOid":"0123456789abcdef0123456789abcdef01234567"}]' "$update" false)"
    if [[ "$(grep -F -c 'callisto release-pr verify' <<< "$output")" -lt 3 ]]; then
      echo "FAIL: failed update must still verify the applied head: $output"
      fail=1
    fi
  else
    output="$(run_case '[]' "$create" false)"
    assert_verify_flags 'failed create' "$output" ''
  fi
  unset VERIFY_FAIL_AT VERIFY_FAIL_PERSIST
  if [[ "$output" != 1$'\n'* ]] \
    || [[ "$output" != *"refs/heads/callisto/version-packages -f sha=$default_new_sha"* ]] \
    || [[ "$output" != *'gh api -X DELETE repos/orin-dx/callisto/git/refs/heads/callisto/version-packages--staging'* ]] \
    || [[ "$output" == *'gh label '* ]] \
    || [[ "$output" == *'gh pr create'* ]] \
    || [[ "$output" == *'gh pr edit'* ]]; then
    echo "FAIL: failed final $kind verification must prevent label and PR writes and clean up staging: $output"
    fail=1
  else
    echo "PASS: failed final $kind verification prevents label and PR writes and cleans up staging"
  fi
done

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

# The hold workflow drafts a stale release PR; a refresh built from the current tip hands it back, one built from an older tip does not.
IS_DRAFT=true
output="$(run_case '[{"number":42,"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"orin-dx/callisto"},"headRefOid":"0123456789abcdef0123456789abcdef01234567"}]' "$update")"
unset IS_DRAFT
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'gh pr ready 42'* ]] \
  || [[ "$output" == *'gh pr ready 42 --undo'* ]]; then
  echo "FAIL: a draft release PR refreshed from the current tip must be marked ready: $output"
  fail=1
else
  echo 'PASS: a refreshed draft release PR is marked ready'
fi

BASE_TIP='ffffffffffffffffffffffffffffffffffffffff'
IS_DRAFT=true
output="$(run_case '[{"number":42,"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"orin-dx/callisto"},"headRefOid":"0123456789abcdef0123456789abcdef01234567"}]' "$update")"
unset IS_DRAFT
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" == *'gh pr ready'* ]]; then
  echo "FAIL: a release PR built from an older tip must stay a draft: $output"
  fail=1
else
  echo 'PASS: a release PR built from an older tip stays a draft'
fi

output="$(run_case '[{"number":42,"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"orin-dx/callisto"},"headRefOid":"0123456789abcdef0123456789abcdef01234567"}]' "$update")"
unset BASE_TIP
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'gh pr ready 42 --undo'* ]]; then
  echo "FAIL: a ready release PR built from an older tip must be drafted: $output"
  fail=1
else
  echo 'PASS: a ready release PR built from an older tip is drafted'
fi

IS_DRAFT=true
output="$(run_case '[]' "$create")"
unset IS_DRAFT
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'gh pr ready 99'* ]]; then
  echo "FAIL: a new release PR is created as a draft and marked ready once built from the current tip: $output"
  fail=1
else
  echo 'PASS: a new release PR is created as a draft and marked ready from the current tip'
fi

BASE_TIP='ffffffffffffffffffffffffffffffffffffffff'
IS_DRAFT=true
output="$(run_case '[]' "$create")"
unset BASE_TIP IS_DRAFT
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" == *'gh pr ready'* ]]; then
  echo "FAIL: a new release PR built from an older tip must stay a draft: $output"
  fail=1
else
  echo 'PASS: a new release PR built from an older tip stays a draft'
fi

# An outage that starts after staging is reset must not make the reset's own retries run the reset again.
TRANSIENT_ON=' api '
TRANSIENT_SKIP=1
TRANSIENT_FAILS=100000
output="$(run_case '[]' "$create")"
unset TRANSIENT_ON TRANSIENT_SKIP TRANSIENT_FAILS
api_calls="$(grep -c '^gh api' <<< "$output" || true)"
if [[ "$output" != 1$'\n'* ]] || [[ "$api_calls" -gt 30 ]]; then
  echo "FAIL: a sustained transient outage must fail within the retry budget, not recurse ($api_calls gh api calls): $output"
  fail=1
else
  echo "PASS: a sustained transient outage stays within the retry budget ($api_calls gh api calls)"
fi

# A failed read of the open PRs stops the action with gh's own status, even where the snapshot is an argument.
PR_LIST_FAIL_AFTER=1
output="$(run_case '[]' "$create")"
unset PR_LIST_FAIL_AFTER
if [[ "$output" != 4$'\n'* ]] \
  || [[ "$output" == *'gh api -X PATCH'*'--staging'* ]]; then
  echo "FAIL: a failed PR read before verification must stop the action with gh's own status before any write: $output"
  fail=1
else
  echo 'PASS: a failed PR read before verification fails the action'
fi

# A transient GitHub error on the commit mutation is retried, and each retry first puts the staging branch back on the base commit.
TRANSIENT_ON=' api graphql '
TRANSIENT_FAILS=2
output="$(run_case '[]' "$create")"
unset TRANSIENT_ON TRANSIENT_FAILS
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$(grep -F -c 'gh api graphql' <<< "$output")" != 3 ]] \
  || [[ "$(grep -F -c -- "-X PATCH repos/orin-dx/callisto/git/refs/heads/callisto/version-packages--staging -f sha=0123456789abcdef0123456789abcdef01234567" <<< "$output")" != 3 ]] \
  || [[ "$output" != *'gh pr create'* ]]; then
  echo "FAIL: a transient GitHub error on the commit mutation must be retried after resetting staging: $output"
  fail=1
else
  echo 'PASS: a transient error on the commit mutation is retried after resetting staging'
fi

# A transient error that never clears stops after four attempts and writes nothing.
TRANSIENT_ON=' pr list '
TRANSIENT_FAILS=100
output="$(run_case '[]' "$create")"
unset TRANSIENT_ON TRANSIENT_FAILS
if [[ "$output" != 1$'\n'* ]] \
  || [[ "$(grep -F -c 'gh pr list' <<< "$output")" != 4 ]] \
  || [[ "$output" == *'gh api'* ]] \
  || [[ "$output" == *'gh pr create'* ]]; then
  echo "FAIL: a persistent transient error must stop after four attempts without writes: $output"
  fail=1
else
  echo 'PASS: a persistent transient error stops after four attempts'
fi

# A failure that is not transient is not retried.
TRANSIENT_ON=' api graphql '
TRANSIENT_FAILS=100
TRANSIENT_MESSAGE='Validation Failed (HTTP 422)'
output="$(run_case '[]' "$create")"
unset TRANSIENT_ON TRANSIENT_FAILS TRANSIENT_MESSAGE
if [[ "$output" != 1$'\n'* ]] \
  || [[ "$(grep -F -c 'gh api graphql' <<< "$output")" != 1 ]] \
  || [[ "$output" == *'gh pr create'* ]]; then
  echo "FAIL: a non-transient error must not be retried: $output"
  fail=1
else
  echo 'PASS: a non-transient error is not retried'
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
