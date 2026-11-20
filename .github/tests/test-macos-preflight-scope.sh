#!/usr/bin/env bash
# Behavior test for .github/scripts/macos-preflight-scope.sh: which changed paths pull in the macOS preflights.
set -u
script="$(cd "$(dirname "${BASH_SOURCE[0]}")/../scripts" && pwd)/macos-preflight-scope.sh"
fail=0

check() { # <expected> <label> <changed paths...>
  local want="$1" label="$2" got
  shift 2
  got="$(printf '%s\n' "$@" | bash "$script")"
  if [[ "$got" != "$want" ]]; then
    printf 'FAIL %s: want %s, got %s\n' "$label" "$want" "${got:-<empty>}" >&2
    fail=1
  fi
}

check true 'release workflow' .github/workflows/callisto-release.yml
check true 'ci workflow' .github/workflows/callisto-ci.yml
check true 'preflight script' .github/scripts/build-release-artifact.sh
check true 'composite action' .github/actions/setup-callisto/action.yml
check true 'workflow template' crates/callisto-graph/src/templates/simple_workflow.tmpl
check true 'root manifest' Cargo.toml
check true 'crate manifest' crates/callisto-cli/Cargo.toml
check true 'lockfile' Cargo.lock
check true 'toolchain pin' rust-toolchain.toml
check true 'release config' callisto.toml
check true 'one relevant among many' README.md crates/callisto-model/src/lib.rs Cargo.lock
check false 'rust source' crates/callisto-model/src/lib.rs
check false 'docs' docs/specs/release.md README.md
check false 'other workflow' .github/workflows/callisto-quality.yml
check false 'nested config look-alike' proto/callisto.toml
check false 'no changes'

# A producer larger than the pipe buffer must not fail the pipeline when a match comes first.
if ! got="$( (set -o pipefail; { echo Cargo.lock; seq 1 200000; } | bash "$script") )" || [[ "$got" != true ]]; then
  printf 'FAIL large diff: got %s\n' "${got:-<empty>}" >&2
  fail=1
fi

exit "$fail"
