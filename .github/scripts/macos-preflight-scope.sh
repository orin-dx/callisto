#!/usr/bin/env bash
# Reads changed paths on stdin and prints true when any of them can change a release artifact build, else false.
set -euo pipefail

while IFS= read -r changed; do
  case "$changed" in
    .github/workflows/callisto-release.yml | .github/workflows/callisto-ci.yml | .github/scripts/build-release-artifact.sh | \
      .github/actions/* | crates/callisto-graph/src/templates/* | \
      Cargo.toml | Cargo.lock | callisto.toml | rust-toolchain* | */Cargo.toml)
      # Drain the rest so a large diff upstream does not die on SIGPIPE under pipefail.
      cat > /dev/null
      echo true
      exit 0
      ;;
  esac
done
echo false
