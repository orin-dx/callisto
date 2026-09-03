#!/usr/bin/env bash
# Black-box regression coverage for the release-PR action implementation. The
# real executable script is run with fake process boundaries, so this checks
# observable GitHub/Git behavior without talking to a repository.
set -euo pipefail

action_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="$action_dir/scripts/create-or-update-release-pr.sh"

run_case() {
  local prs="$1" label_exists="$2" remote_branch_exists="$3" workflow_changed="$4" status_code="$5"
  local calls_file output_file harness
  calls_file="$(mktemp)"
  output_file="$(mktemp)"
  harness="$(mktemp)"

  {
    cat <<'STUBS'
callisto() {
  case "$1" in
    status) return "$STATUS_CODE" ;;
    matrix) printf '%s\n' '{"platformTargets":[]}' ;;
    compose-pr-body) echo 'composed release body' ;;
    version) return 0 ;;
    *) echo "unexpected callisto invocation: $*" >&2; return 97 ;;
  esac
}
gh() {
  echo "gh $*" >> "$CALLS_FILE"
  if [[ "$1 $2" == 'pr list' ]]; then
    local query="${!#}"
    jq "$query" <<< "$PR_LIST"
    return 0
  fi
  if [[ "$1" == api ]]; then echo "$LABEL_EXISTS"; return 0; fi
  if [[ "$1 $2" == 'label list' ]]; then echo "$LABEL_EXISTS"; return 0; fi
  if [[ "$1 $2" == 'pr create' ]]; then echo 'https://github.com/orin-dx/callisto/pull/99'; return 0; fi
}
git() {
  echo "git $*" >> "$CALLS_FILE"
  case "$1" in
    status) echo ' M Cargo.toml'; return 0 ;;
    ls-remote) [[ "$REMOTE_BRANCH_EXISTS" == true ]] && return 0 || return 2 ;;
    diff) [[ "$WORKFLOW_CHANGED" == true ]] && return 1 || return 0 ;;
    rev-parse) echo "$BASE_SHA"; return 0 ;;
  esac
  return 0
}
STUBS
  } > "$harness"

  set +e
  CALLS_FILE="$calls_file" PR_LIST="$prs" LABEL_EXISTS="$label_exists" \
    REMOTE_BRANCH_EXISTS="$remote_branch_exists" WORKFLOW_CHANGED="$workflow_changed" \
    BASE_SHA='0123456789abcdef0123456789abcdef01234567' STATUS_CODE="$status_code" GITHUB_OUTPUT="$output_file" \
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

fail=0
output="$(run_case '[]' true false false 1)"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'gh pr create --head callisto/version-packages --base main'* ]] \
  || [[ "$output" != *'git push --force-with-lease origin callisto/version-packages'* ]]; then
  echo "FAIL: creates exactly the configured release PR branch: $output"
  fail=1
else
  echo 'PASS: creates the configured release PR branch'
fi

output="$(run_case '[{"number":1,"body":"first","headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"orin-dx/callisto"}},{"number":2,"body":"second","headRefName":"callisto/version-packages--0123456789abcdef0123456789abcdef01234567","headRepository":{"nameWithOwner":"orin-dx/callisto"}}]' true false false 1)"
if [[ "$output" != 1$'\n'* ]] || [[ "$output" == *'git switch'* ]]; then
  echo "FAIL: ambiguous open release PRs must fail before mutation: $output"
  fail=1
else
  echo 'PASS: rejects ambiguous release PR state'
fi

output="$(run_case '[{"number":7,"body":"foreign","headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"fork/callisto"}},{"number":8,"body":"malformed","headRefName":"callisto/version-packages--short","headRepository":{"nameWithOwner":"orin-dx/callisto"}}]' true false false 1)"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'gh pr create --head callisto/version-packages --base main'* ]] \
  || [[ "$output" == *'gh pr edit 7'* ]] \
  || [[ "$output" == *'gh pr edit 8'* ]]; then
  echo "FAIL: foreign or malformed managed-branch lookalikes must not be selected: $output"
  fail=1
else
  echo 'PASS: rejects foreign and malformed managed-branch lookalikes'
fi

output="$(run_case '[{"number":42,"body":"existing","headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"orin-dx/callisto"}}]' false true false 1)"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'gh label create callisto: release'* ]] \
  || [[ "$output" != *'gh pr edit 42'* ]] \
  || [[ "$output" == *'gh pr create'* ]]; then
  echo "FAIL: updates the sole PR and propagates label failures: $output"
  fail=1
else
  echo 'PASS: updates the sole PR and creates a missing label'
fi

output="$(run_case '[]' true true true 1)"
fallback_branch='callisto/version-packages--0123456789abcdef0123456789abcdef01234567'
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *"git switch -C $fallback_branch"* ]] \
  || [[ "$output" != *"git push --force-with-lease origin $fallback_branch"* ]] \
  || [[ "$output" != *"gh pr create --head $fallback_branch --base main"* ]]; then
  echo "FAIL: workflow-history fallback must create a SHA-suffixed managed branch: $output"
  fail=1
else
  echo 'PASS: rotates to a SHA-suffixed branch for workflow history'
fi

output="$(run_case '[{"number":42,"body":"existing","headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"orin-dx/callisto"}}]' true true true 1)"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'gh pr create --head callisto/version-packages--0123456789abcdef0123456789abcdef01234567 --base main'* ]] \
  || [[ "$output" != *'gh pr close 42 --comment Superseded by https://github.com/orin-dx/callisto/pull/99'* ]] \
  || [[ "$output" == *'gh pr edit 42'* ]]; then
  echo "FAIL: fallback must replace, link, and close the superseded PR: $output"
  fail=1
else
  echo 'PASS: replaces and links the superseded PR only for workflow history'
fi

output="$(run_case '[{"number":99,"body":"replacement","headRefName":"callisto/version-packages--0123456789abcdef0123456789abcdef01234567","headRepository":{"nameWithOwner":"orin-dx/callisto"}}]' true true false 1)"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *"git push --force-with-lease origin $fallback_branch"* ]] \
  || [[ "$output" != *'gh pr edit 99'* ]] \
  || [[ "$output" == *'gh pr create'* ]]; then
  echo "FAIL: later ordinary changes must retain the replacement branch and PR: $output"
  fail=1
else
  echo 'PASS: retains the replacement PR after ordinary changes'
fi

output="$(run_case '[]' true false false 2)"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" == *'git switch'* ]] \
  || [[ "$output" == *'gh pr '* ]]; then
  echo "FAIL: no changesets must stop before GitHub or Git mutation: $output"
  fail=1
else
  echo 'PASS: no changesets stops before GitHub or Git mutation'
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
  echo 'FAIL: label lookup must not combine gh api --slurp with --jq'
  fail=1
else
  echo 'PASS: label lookup avoids incompatible gh api output flags'
fi

if [[ "$script_contents" == *'gh label create'*'|| true'* ]]; then
  echo 'FAIL: label-creation errors are hidden'
  fail=1
else
  echo 'PASS: label-creation errors are not hidden'
fi

exit "$fail"
