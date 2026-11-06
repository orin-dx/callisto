#!/usr/bin/env bash
# Keeps one comment on the release PR that says why it is held, so a draft needs no trip to the Actions tab.
# Source it and call `release_pr_note`, or run it: release-pr-note.sh <held|ready|failed> <pr-number> [run-url]
# Needs GH_TOKEN and GITHUB_REPOSITORY; GITHUB_SHA names the push the note is about.
set -euo pipefail

release_pr_note() {
  local state="$1" pr="$2" run_url="${3:-}"
  local marker='<!-- callisto-release-pr-state -->'
  local sha="${GITHUB_SHA:-}" body existing
  local short="${sha:0:7}"
  case "$state" in
    held)
      body="Held as a draft: \`main\` moved to \`$short\`, so this PR may be stale. The Release workflow marks it ready after it rebuilds the PR from the new tip."
      ;;
    ready)
      body="Ready: rebuilt from \`main\` at \`$short\`."
      ;;
    failed)
      body="Still a draft: the Release workflow could not rebuild this PR from \`main\` at \`$short\`. See ${run_url:-the Release workflow runs}, fix the cause and re-run it; the PR becomes ready when the rebuild succeeds."
      ;;
    *)
      echo "unknown release PR state: $state" >&2
      return 2
      ;;
  esac
  body="$marker"$'\n'"$body"
  # Callers run this on the left of `||`, where errexit is off, so each step returns its own failure. A failed
  # lookup must not read as "no comment yet", or the POST below would add a second one.
  existing=$(gh api --paginate "repos/$GITHUB_REPOSITORY/issues/$pr/comments" \
    --jq ".[] | select(.body | startswith(\"$marker\")) | .id" | sed -n 1p) || {
    echo "::warning::could not read the comments on #$pr, so its note is left as it is" >&2
    return 1
  }
  if [[ -n "$existing" ]]; then
    gh api -X PATCH "repos/$GITHUB_REPOSITORY/issues/comments/$existing" -f body="$body" > /dev/null || return 1
  else
    gh api -X POST "repos/$GITHUB_REPOSITORY/issues/$pr/comments" -f body="$body" > /dev/null || return 1
  fi
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  release_pr_note "${1:?usage: release-pr-note.sh <held|ready|failed> <pr-number> [run-url]}" "${2:?pr number}" "${3:-}"
fi
