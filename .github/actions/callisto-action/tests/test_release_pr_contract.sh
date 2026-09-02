#!/usr/bin/env bash
# Black-box regression coverage for the release-PR composite action. The
# action's YAML run block is executed with fake process boundaries, so this
# checks observable GitHub/Git behavior without talking to a repository.
set -euo pipefail

action_yml="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/action.yml"

extract_run() {
  sed -n '/set -euo pipefail/,$p' "$action_yml"
}

run_case() {
  local prs="$1" label_exists="$2"
  local calls_file output_file script
  calls_file="$(mktemp)"
  output_file="$(mktemp)"
  script="$(mktemp)"

  {
    cat <<'STUBS'
callisto() {
  case "$1" in
    status) return 1 ;;
    compose-pr-body) echo 'composed release body' ;;
    version) return 0 ;;
    *) echo "unexpected callisto invocation: $*" >&2; return 97 ;;
  esac
}
gh() {
  echo "gh $*" >> "$CALLS_FILE"
  if [[ "$1 $2" == 'pr list' ]]; then printf '%s\n' "$PR_LIST"; return 0; fi
  if [[ "$1" == api ]]; then echo "$LABEL_EXISTS"; return 0; fi
}
git() {
  echo "git $*" >> "$CALLS_FILE"
  [[ "$1" == status ]] && { echo ' M Cargo.toml'; return 0; }
  return 0
}
STUBS
    extract_run
  } > "$script"

  set +e
  CALLS_FILE="$calls_file" PR_LIST="$prs" LABEL_EXISTS="$label_exists" GITHUB_OUTPUT="$output_file" \
    INPUT_VERSION_COMMAND='callisto version' \
    INPUT_COMMIT_MESSAGE='chore(release): version packages' \
    INPUT_TITLE='chore(release): version packages' \
    INPUT_PR_LABEL='callisto: release' \
    INPUT_SETUP_GIT_USER=true \
    INPUT_BRANCH=main \
    INPUT_RELEASE_BRANCH='callisto/version-packages' \
    INPUT_CWD=. \
    GITHUB_REPOSITORY='orin-dx/callisto' \
    bash "$script" > /dev/null 2>&1
  local code=$?
  set -e

  printf '%s\n---calls---\n' "$code"
  cat "$calls_file"
  rm -f "$calls_file" "$output_file" "$script"
}

fail=0
output="$(run_case '[]' true)"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'gh pr create --head callisto/version-packages --base main'* ]] \
  || [[ "$output" != *'git push --force-with-lease origin callisto/version-packages'* ]]; then
  echo "FAIL: creates exactly the configured release PR branch: $output"
  fail=1
else
  echo 'PASS: creates the configured release PR branch'
fi

output="$(run_case '[{"number":1,"body":"first"},{"number":2,"body":"second"}]' true)"
if [[ "$output" != 1$'\n'* ]] || [[ "$output" == *'git switch'* ]]; then
  echo "FAIL: ambiguous open release PRs must fail before mutation: $output"
  fail=1
else
  echo 'PASS: rejects ambiguous release PR state'
fi

output="$(run_case '[{"number":42,"body":"existing"}]' false)"
if [[ "$output" != 0$'\n'* ]] \
  || [[ "$output" != *'gh label create callisto: release'* ]] \
  || [[ "$output" != *'gh pr edit 42'* ]] \
  || [[ "$output" == *'gh pr create'* ]]; then
  echo "FAIL: updates the sole PR and propagates label failures: $output"
  fail=1
else
  echo 'PASS: updates the sole PR and creates a missing label'
fi

action_contents="$(<"$action_yml")"
if [[ "$action_contents" == *'gh label create'*'|| true'* ]]; then
  echo 'FAIL: label-creation errors are still hidden'
  fail=1
else
  echo 'PASS: label-creation errors are not hidden'
fi

exit "$fail"
