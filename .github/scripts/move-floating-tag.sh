#!/usr/bin/env bash
# Moves the floating major tag (v1, v2, ...) to a release commit, for a stable release at or past 1.0.0.
# Run inside the release-source checkout, after the release succeeded, so `uses: ...@v1` follows the newest 1.x.
# Usage: move-floating-tag.sh <release-decision.json> <package> <commit> [remote]
set -euo pipefail

decision="${1:?usage: move-floating-tag.sh <release-decision.json> <package> <commit> [remote]}"
package="${2:?missing package}"
commit="${3:?missing commit}"
remote="${4:-origin}"

if [[ ! "$commit" =~ ^[0-9a-f]{40}$ ]]; then
  echo "::error::commit must be exactly 40 lowercase hex characters"
  exit 1
fi

version=$(jq -r --arg package "$package" '[.entries[] | select(.package == $package) | .targetVersion][0] // empty' "$decision")
if [[ -z "$version" ]]; then
  echo "$package is not in this release; no floating tag to move"
  exit 0
fi
if [[ ! "$version" =~ ^([0-9]+)\.[0-9]+\.[0-9]+$ ]]; then
  echo "$version is not a stable X.Y.Z version; no floating tag to move"
  exit 0
fi
major="${BASH_REMATCH[1]}"
if ((major < 1)); then
  echo "$version is before 1.0.0; no floating tag to move"
  exit 0
fi
tag="v$major"

current=$(git ls-remote --tags "$remote" "refs/tags/$tag" | awk 'NR == 1 { print $1 }')
if [[ "$current" == "$commit" ]]; then
  echo "$tag already points at $commit"
  exit 0
fi
if [[ -n "$current" ]]; then
  # A recovery run of an older release must not drag the tag backwards.
  git fetch --quiet "$remote" "refs/tags/$tag:refs/tags/$tag" --force
  if ! git merge-base --is-ancestor "$current" "$commit"; then
    echo "$tag points at $current, which is not an ancestor of $commit; leaving it"
    exit 0
  fi
fi

git tag --force "$tag" "$commit"
git push --force "$remote" "refs/tags/$tag"
echo "$tag now points at $commit ($package $version)"
