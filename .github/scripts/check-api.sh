#!/usr/bin/env bash
# Fails when the public API changed more than the pending changesets allow.
# A feature PR has not bumped the version yet, so the release type comes from the pending changesets; the release PR
# has consumed them and its bumped version decides.
set -euo pipefail

root=${CHECK_API_ROOT:-$(git rev-parse --show-toplevel)}
cd "$root"

level=''
shopt -s nullglob
for file in .changeset/*.md; do
  [[ $file == */README.md ]] && continue
  while IFS= read -r bump; do
    if [[ $bump == major ]]; then
      level=major
    elif [[ $bump == minor && $level != major ]]; then
      level=minor
    elif [[ $bump == patch && -z $level ]]; then
      level='patch'
    fi
  done < <(awk '/^---$/ { fences++; next } fences == 1' "$file" | sed -nE 's/^[^:]*: *(major|minor|patch) *$/\1/p')
done

if [[ -z $level ]]; then
  cargo semver-checks check-release
  exit
fi

# Before 1.0 a minor release may break the API and a patch release may only add to it, which cargo-semver-checks names one step up.
major_version=$(sed -n 's/^version = "\([0-9]*\)\..*/\1/p' crates/callisto-cli/Cargo.toml | head -n1)
release_type=$level
if [[ $major_version == 0 ]]; then
  case $level in
    patch) release_type=minor ;;
    minor | major) release_type=major ;;
  esac
fi
echo "pending changesets allow a $level release; checking the API as a $release_type change"
cargo semver-checks check-release --release-type "$release_type"
