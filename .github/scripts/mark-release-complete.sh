#!/usr/bin/env bash
# Change a release PR's advisory marker only after the executor succeeded.
set -euo pipefail

pr="$1"
sha="$2"
[[ "$pr" =~ ^[1-9][0-9]*$ && "$sha" =~ ^[0-9a-f]{40}$ ]] || {
  echo 'release PR number or source SHA is invalid' >&2
  exit 1
}

details=$(gh pr view "$pr" --json mergedAt,mergeCommit,labels)
if ! jq -e --arg sha "$sha" '.mergedAt != null and .mergeCommit.oid == $sha' <<< "$details" > /dev/null; then
  echo "release PR #$pr does not identify source commit $sha" >&2
  exit 1
fi

pending=$(jq -r 'any(.labels[]; .name == "callisto: pending")' <<< "$details")
complete=$(jq -r 'any(.labels[]; .name == "callisto: complete")' <<< "$details")
if [[ "$complete" != true ]]; then
  exists=$(gh label list --limit 1000 --json name --jq 'any(.[]; .name == "callisto: complete")')
  if [[ "$exists" != true ]]; then
    gh label create 'callisto: complete' --color 0e8a16 --description 'Every planned release operation was verified complete'
  fi
fi

args=()
[[ "$pending" == true ]] && args+=(--remove-label 'callisto: pending')
[[ "$complete" != true ]] && args+=(--add-label 'callisto: complete')
if [[ ${#args[@]} -gt 0 ]]; then
  gh pr edit "$pr" "${args[@]}"
fi
