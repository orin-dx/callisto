#!/usr/bin/env bash
# Regression test for a real production incident (2026-08-27): consumer CI
# (orin-dx/michi) failed to install callisto because setup-callisto's
# fallback chain never tried crates.io -- only a GitHub Release binary
# tarball (which has no assets attached to any release, always 404s) and
# `cargo install --git` (broken on main by an unrelated yanked dependency).
# callisto-cli@0.5.0 was already published and installable from crates.io
# the whole time. This proves the new crates.io fallback is actually tried,
# in the right order, with the right args, before the git-source fallback.
set -u
ACTION_YML="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/action.yml"

extract_snippet() {
  # Starts after the TAG_NAME="${{ inputs.version || 'latest' }}" line (raw
  # GitHub Actions expression syntax, not valid bash on its own) -- the
  # caller sets TAG_NAME directly instead. Uses explicit line numbers, not a
  # `fi`-pattern range, because the block contains a nested if/elif/fi (OS
  # detection) before the outer if/elif/elif/else/fi this test targets --
  # a pattern-based range would stop at the first (inner) `fi` it finds.
  local start end
  start=$(grep -n '# Detect OS platform architecture for pre-built binaries' "$ACTION_YML" | head -1 | cut -d: -f1)
  end=$(grep -n 'CALLISTO_BIN_DIR" >> \$GITHUB_PATH' "$ACTION_YML" | head -1 | cut -d: -f1)
  end=$((end - 1))
  sed -n "${start},${end}p" "$ACTION_YML"
}

run_case() {
  local version="$1"
  local curl_behavior="$2"    # "fail" or "succeed"
  local cargo_behavior="$3"   # "crates-io-succeeds" or "crates-io-fails"
  local calls_file tmp_script workdir
  calls_file="$(mktemp)"
  workdir="$(mktemp -d)"
  tmp_script="$(mktemp)"
  {
    echo "curl() { echo \"curl \$*\" >> '${calls_file}'; [[ '${curl_behavior}' == fail ]] && return 1 || return 0; }"
    echo "tar() { return 0; }"
    echo "cargo() {"
    echo "  echo \"cargo \$*\" >> '${calls_file}'"
    echo "  if [[ \"\$1\" == install && \"\$*\" == *'callisto-cli'* && \"\$*\" != *'--git'* ]]; then"
    echo "    [[ '${cargo_behavior}' == crates-io-succeeds ]] && { mkdir -p \"\${RUNNER_TEMP}/callisto-cargo-install/bin\"; touch \"\${RUNNER_TEMP}/callisto-cargo-install/bin/callisto\"; return 0; } || return 1"
    echo "  fi"
    echo "  if [[ \"\$*\" == *'--git'* ]]; then"
    echo "    mkdir -p \"\${RUNNER_TEMP}/callisto-cargo-install/bin\"; touch \"\${RUNNER_TEMP}/callisto-cargo-install/bin/callisto\"; return 0"
    echo "  fi"
    echo "}"
    echo "cp() { command cp \"\$@\"; }"
    echo "mkdir() { command mkdir \"\$@\"; }"
    echo "grep() { command grep \"\$@\"; }"
    echo "uname() { [[ \"\$1\" == -s ]] && echo Linux || echo x86_64; }"
    echo "RUNNER_TEMP='${workdir}'"
    echo "GITHUB_WORKSPACE='${workdir}/not-callisto'"
    echo "mkdir -p \"\$GITHUB_WORKSPACE\""
    echo "GITHUB_PATH='${workdir}/github_path'"
    echo "touch \"\$GITHUB_PATH\""
    echo "TAG_NAME='${version}'"
    echo "CALLISTO_BIN_DIR='${workdir}/callisto-bin'"
    echo "mkdir -p '${workdir}/callisto-bin'"
    extract_snippet
  } > "$tmp_script"
  bash "$tmp_script" 2>/dev/null
  local code=$?
  cat "$calls_file"
  rm -f "$tmp_script" "$calls_file"
  rm -rf "$workdir"
  return $code
}

fail=0

# crates.io install must be attempted (and succeed) before the git fallback,
# when the GH-release binary download fails and version="latest".
out=$(run_case "latest" "fail" "crates-io-succeeds")
if [[ "$out" != *"cargo install callisto-cli --locked"* ]]; then
  echo "FAIL: crates.io install (latest) must be attempted, got: $out"; fail=1
elif [[ "$out" == *"--git"* ]]; then
  echo "FAIL: git fallback must not run when crates.io install succeeds, got: $out"; fail=1
else
  echo "PASS: crates.io install (latest) attempted, git fallback skipped"
fi

# An explicit "callisto-cli@0.5.0" version must strip to a bare "0.5.0" for
# `cargo install --version`.
out=$(run_case "callisto-cli@0.5.0" "fail" "crates-io-succeeds")
if [[ "$out" != *"--version 0.5.0"* ]]; then
  echo "FAIL: explicit version must strip the callisto-cli@ prefix, got: $out"; fail=1
else
  echo "PASS: explicit version tag strips to bare semver for --version"
fi

# When crates.io install also fails, the git-source fallback must still run
# (it isn't removed, just demoted to last resort).
out=$(run_case "latest" "fail" "crates-io-fails")
if [[ "$out" != *"--git"* ]]; then
  echo "FAIL: git fallback must still run when crates.io install fails too, got: $out"; fail=1
else
  echo "PASS: git fallback still runs when crates.io install fails"
fi

exit $fail
