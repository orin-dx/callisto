#!/usr/bin/env bash
# With no verification input the real script refuses an unverifiable asset, names the opt-out and never installs from crates.io.
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/callisto.XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/shims" "$work/ws"
printf '#!/bin/sh\nexit 0\n' > "$work/shims/curl"
printf '#!/bin/sh\nexit 1\n' > "$work/shims/gh"
printf '#!/bin/sh\necho "$@" >> "%s/cargo.calls"\nexit 1\n' "$work" > "$work/shims/cargo"
printf '#!/bin/sh\ncase "$1" in -s) echo Linux;; *) echo x86_64;; esac\n' > "$work/shims/uname"
chmod +x "$work/shims"/*
out=$(env -u INPUT_CALLISTO_VERIFICATION PATH="$work/shims:$PATH" RUNNER_TEMP="$work" GITHUB_WORKSPACE="$work/ws" \
  GITHUB_PATH="$work/path" INPUT_CALLISTO_SOURCE=auto INPUT_CALLISTO_VERSION=callisto@0.5.0 \
  bash "$ROOT/scripts/install-callisto.sh" 2>&1); code=$?
if [[ $code -ne 0 && "$out" == *"verification: fallback"* && ! -e "$work/cargo.calls" ]]; then
  echo "PASS: default verification refuses an unverified asset and names the opt-out"
else
  echo "FAIL: default must require verification: code=$code out=$out"; exit 1
fi
