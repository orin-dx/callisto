#!/usr/bin/env bash
# Writes how the release PR rebuild ended on the PR's state comment (see hold-release-pr.sh for how it got held).
# Usage: note-release-pr-rebuild.sh <release-branch> <success|failure> <run-url>
# Needs GH_TOKEN, GITHUB_REPOSITORY and GITHUB_SHA. A rebuild that ended with the PR still a draft, because main moved
# again, leaves the hold note: a newer run is already on its way.
set -euo pipefail

release_branch="${1:?usage: note-release-pr-rebuild.sh <release-branch> <success|failure> <run-url>}"
outcome="${2:?outcome}"
run_url="${3:?run url}"

# shellcheck source-path=SCRIPTDIR source=release-pr-note.sh
source "$(dirname "${BASH_SOURCE[0]}")/release-pr-note.sh"

prs=$(gh pr list --repo "$GITHUB_REPOSITORY" --state open --limit 1000 --json number,isDraft,headRefName,headRepository)
found=$(jq -r --arg repo "$GITHUB_REPOSITORY" --arg branch "$release_branch" '
  [.[] | select((.headRepository.nameWithOwner // "") == $repo
    and (.headRefName == $branch or (.headRefName | startswith($branch + "--"))))][0]
  | if . == null then "" else "\(.number) \(.isDraft)" end' <<< "$prs")
[[ -n "$found" ]] || exit 0
read -r number is_draft <<< "$found"

case "$outcome" in
  success)
    if [[ "$is_draft" == false ]]; then release_pr_note ready "$number"; fi
    ;;
  failure)
    release_pr_note failed "$number" "$run_url"
    ;;
esac
