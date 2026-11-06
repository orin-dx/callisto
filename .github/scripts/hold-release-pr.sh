#!/usr/bin/env bash
# Drafts the open managed release PR as soon as main moves, because a draft cannot be merged. The release
# workflow marks it ready again only after a refresh built from the new tip, so a failed or skipped refresh
# leaves a stale release PR unmergeable instead of mergeable. Usage: hold-release-pr.sh <release-branch>
# Needs GH_TOKEN and GITHUB_REPOSITORY.
set -euo pipefail

release_branch="${1:?usage: hold-release-pr.sh <release-branch>}"

# shellcheck source-path=SCRIPTDIR source=release-pr-note.sh
source "$(dirname "${BASH_SOURCE[0]}")/release-pr-note.sh"

prs=$(gh pr list --repo "$GITHUB_REPOSITORY" --state open --limit 1000 --json number,isDraft,headRefName,headRepository)
numbers=$(jq -r --arg repo "$GITHUB_REPOSITORY" --arg branch "$release_branch" '
  .[] | select(.isDraft == false
    and (.headRepository.nameWithOwner // "") == $repo
    and (.headRefName == $branch or (.headRefName | startswith($branch + "--")))) | .number' <<< "$prs")

for number in $numbers; do
  for attempt in 1 2 3; do
    if gh pr ready "$number" --repo "$GITHUB_REPOSITORY" --undo; then
      echo "::notice::Held release PR #$number as a draft until the release workflow refreshes it"
      release_pr_note held "$number" || echo "::warning::could not write the hold note on release PR #$number"
      break
    fi
    [[ $attempt -lt 3 ]] || { echo "::error::could not draft release PR #$number"; exit 1; }
    sleep $((attempt * 2))
  done
done
