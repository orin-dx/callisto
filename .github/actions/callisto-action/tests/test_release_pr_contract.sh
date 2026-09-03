#!/usr/bin/env bash
# Black-box coverage for the exact release-PR action executable. The fake
# process boundary proves the script executes only Callisto's closed decision
# set; Rust tests cover decision policy itself.
set -euo pipefail

action_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="$action_dir/scripts/create-or-update-release-pr.sh"

run_case() {
  local prs="$1" decision="$2" label_exists="$3" workflow_changed="$4" verify_code="$5"
  local calls_file output_file harness
  calls_file="$(mktemp)"
  output_file="$(mktemp)"
  harness="$(mktemp)"

  {
    cat <<'STUBS'
callisto() {
  echo "callisto $*" >> "$CALLS_FILE"
  if [[ " $* " == *' release-pr decide '* ]]; then printf '%s\n' "$DECISION"; return 0; fi
  if [[ " $* " == *' release-pr verify '* ]]; then return "$VERIFY_CODE"; fi
  case "$1" in
    matrix) printf '%s\n' '{"platformTargets":[]}' ;;
    compose-pr-body) echo 'composed release body' ;;
    version) return 0 ;;
    *) echo "unexpected callisto invocation: $*" >&2; return 97 ;;
  esac
}
gh() {
  echo "gh $*" >> "$CALLS_FILE"
  if [[ "$1 $2" == 'pr list' ]]; then printf '%s\n' "$PR_LIST"; return 0; fi
  if [[ "$1 $2" == 'pr view' ]]; then echo 'existing body'; return 0; fi
  if [[ "$1 $2" == 'label list' ]]; then echo "$LABEL_EXISTS"; return 0; fi
  if [[ "$1 $2" == 'pr create' ]]; then echo 'https://github.com/orin-dx/callisto/pull/99'; return 0; fi
}
git() {
  echo "git $*" >> "$CALLS_FILE"
  case "$1" in
    status) echo ' M Cargo.toml'; return 0 ;;
    rev-parse) echo "$BASE_SHA"; return 0 ;;
    check-ref-format|fetch) return 0 ;;
    diff) [[ "$WORKFLOW_CHANGED" == true ]] && return 1 || return 0 ;;
  esac
  return 0
}
STUBS
  } > "$harness"

  set +e
  CALLS_FILE="$calls_file" PR_LIST="$prs" DECISION="$decision" LABEL_EXISTS="$label_exists" \
    WORKFLOW_CHANGED="$workflow_changed" VERIFY_CODE="$verify_code" \
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
  rm -f "$calls_file" "$output_file" "$harness"
}

create='{"schemaVersion":1,"action":{"kind":"create","branch":"callisto/version-packages"}}'
update='{"schemaVersion":1,"action":{"kind":"update","pullRequestNumber":42,"branch":"callisto/version-packages"}}'
fallback_branch='callisto/version-packages--0123456789abcdef0123456789abcdef01234567'
supersede="{\"schemaVersion\":1,\"action\":{\"kind\":\"supersede\",\"pullRequestNumber\":42,\"expectedBranch\":\"callisto/version-packages\",\"replacementBranch\":\"$fallback_branch\"}}"
retained="{\"schemaVersion\":1,\"action\":{\"kind\":\"update\",\"pullRequestNumber\":99,\"branch\":\"$fallback_branch\"}}"
noop='{"schemaVersion":1,"action":{"kind":"noop","reason":{"kind":"noPendingChangesets"}}}'

fail=0
output="$(run_case '[]' "$create" true false 0)"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'callisto --format json release-pr decide'* ]] \
  || [[ "$(rg -F -c 'callisto release-pr verify' <<< "$output")" != 2 ]] \
  || [[ "$output" != *'gh pr create --head callisto/version-packages --base main'* ]]; then
  echo "FAIL: executes Callisto create decision with both mutation-boundary checks: $output"
  fail=1
else
  echo 'PASS: executes Callisto create decision'
fi

output="$(run_case '[{"number":42,"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"orin-dx/callisto"}}]' "$update" false false 0)"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'gh label create callisto: release'* ]] \
  || [[ "$output" != *'gh pr edit 42'* ]] \
  || [[ "$output" == *'gh pr create'* ]]; then
  echo "FAIL: executes Callisto update decision: $output"
  fail=1
else
  echo 'PASS: executes Callisto update decision'
fi

output="$(run_case '[{"number":42,"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"orin-dx/callisto"}}]' "$supersede" true true 0)"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *"git push --force-with-lease origin $fallback_branch"* ]] \
  || [[ "$output" != *"gh pr create --head $fallback_branch --base main"* ]] \
  || [[ "$output" != *'gh pr close 42 --comment Superseded by https://github.com/orin-dx/callisto/pull/99'* ]] \
  || [[ "$output" == *'gh pr edit 42'* ]]; then
  echo "FAIL: executes Callisto supersede decision: $output"
  fail=1
else
  echo 'PASS: executes Callisto supersede decision'
fi

output="$(run_case "[{\"number\":99,\"headRefName\":\"$fallback_branch\",\"headRepository\":{\"nameWithOwner\":\"orin-dx/callisto\"}}]" "$retained" true false 0)"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *"git push --force-with-lease origin $fallback_branch"* ]] \
  || [[ "$output" != *'gh pr edit 99'* ]] \
  || [[ "$output" == *'gh pr create'* ]]; then
  echo "FAIL: retains Callisto-selected replacement PR: $output"
  fail=1
else
  echo 'PASS: retains Callisto-selected replacement PR'
fi

output="$(run_case '[]' "$noop" true false 0)"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" == *'git switch'* ]] \
  || [[ "$output" == *'git push'* ]] \
  || [[ "$output" == *'gh pr create'* ]] \
  || [[ "$output" == *'gh pr edit'* ]]; then
  echo "FAIL: no-op decision must stop before mutation: $output"
  fail=1
else
  echo 'PASS: no-op decision stops before mutation'
fi

output="$(run_case '[]' "$create" true false 1)"
if [[ "$output" != 1$'\n'* ]] \
  || [[ "$output" == *'git push'* ]] \
  || [[ "$output" == *'gh label '* ]] \
  || [[ "$output" == *'gh pr create'* ]] \
  || [[ "$output" == *'gh pr edit'* ]] \
  || [[ "$output" == *'gh pr close'* ]]; then
  echo "FAIL: stale snapshot must prevent every forge mutation: $output"
  fail=1
else
  echo 'PASS: stale snapshot prevents forge mutation'
fi

action_contents="$(<"$action_dir/action.yml")"
if [[ "$action_contents" != *'bash "$GITHUB_ACTION_PATH/scripts/create-or-update-release-pr.sh"'* ]]; then
  echo 'FAIL: action metadata does not invoke the tested implementation script'
  fail=1
else
  echo 'PASS: action metadata invokes the tested implementation script'
fi

script_contents="$(<"$script")"
if [[ "$script_contents" == *'gh api --paginate'*'--slurp'*'--jq'* ]]; then
  echo 'FAIL: label lookup combines incompatible gh api --slurp and --jq flags'
  fail=1
elif [[ "$script_contents" != *'callisto --format json release-pr decide'* ]] \
  || [[ "$script_contents" != *'callisto release-pr verify'* ]]; then
  echo 'FAIL: action still derives release-PR policy outside Callisto'
  fail=1
else
  echo 'PASS: action delegates policy and uses compatible label lookup'
fi

# The configured branch is policy input to Callisto, not an action-side branch
# matching rule. A second reference would be a strong signal that the adapter
# has started to recreate release-PR policy.
release_branch_reference_count=$(rg -o 'INPUT_RELEASE_BRANCH' "$script" | wc -l | tr -d ' ')
if [[ "$release_branch_reference_count" != 1 ]]; then
  echo 'FAIL: action must pass the configured release branch to Callisto exactly once'
  fail=1
else
  echo 'PASS: action does not recreate managed-branch policy'
fi

exit "$fail"
