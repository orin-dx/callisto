#!/usr/bin/env bash
# check-api.sh picks cargo-semver-checks' release type from the pending changesets.
set -euo pipefail

script=$(cd "$(dirname "$0")/.." && pwd)/scripts/check-api.sh
work=$(mktemp -d "${TMPDIR:-/tmp}/check-api.XXXXXX")
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"
# shellcheck disable=SC2016
printf '#!/bin/sh\necho "cargo $*" >> "$CALLS"\n' > "$work/bin/cargo"
chmod +x "$work/bin/cargo"

fail=0
# usage: expect <name> <crate version> <expected cargo args> [changeset...]; each changeset is "name:level".
expect() {
  local name=$1 version=$2 want=$3
  shift 3
  local root="$work/$name"
  mkdir -p "$root/.changeset" "$root/crates/callisto-cli"
  printf '[package]\nname = "callisto-cli"\nversion = "%s"\n' "$version" > "$root/crates/callisto-cli/Cargo.toml"
  printf '# changesets\n' > "$root/.changeset/README.md"
  local i=0 entry
  for entry in "$@"; do
    i=$((i + 1))
    printf -- '---\ncallisto-cli: %s\n---\n\nSummary %s.\n' "${entry#*:}" "$i" > "$root/.changeset/${entry%%:*}-$i.md"
  done
  : > "$work/calls"
  CALLS="$work/calls" CHECK_API_ROOT="$root" PATH="$work/bin:$PATH" bash "$script" > /dev/null
  local got
  got=$(cat "$work/calls")
  if [[ "$got" == "$want" ]]; then
    echo "PASS $name"
  else
    echo "FAIL $name: want '$want', got '$got'"
    fail=1
  fi
}

expect no_pending_changesets_uses_the_version 0.17.0 'cargo semver-checks check-release'
expect pre_1_patch_is_a_minor_change 0.17.0 'cargo semver-checks check-release --release-type minor' a:patch
expect pre_1_minor_may_break 0.17.0 'cargo semver-checks check-release --release-type major' a:patch b:minor
expect pre_1_major_may_break 0.17.0 'cargo semver-checks check-release --release-type major' a:major b:patch
expect post_1_patch 1.2.0 'cargo semver-checks check-release --release-type patch' a:patch
expect post_1_minor 1.2.0 'cargo semver-checks check-release --release-type minor' a:patch b:minor
expect post_1_major 1.2.0 'cargo semver-checks check-release --release-type major' a:minor b:major
exit "$fail"
