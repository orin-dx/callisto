#!/usr/bin/env bash
# The fallback chain must try crates.io, in order and with the right args; there is no git-source fallback.
set -u
INSTALL_SCRIPT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/scripts/install-callisto.sh"

extract_snippet() {
  # Starts after the TAG_NAME assignment; explicit line numbers because a nested `fi` would end a pattern range early.
  local start end
  start=$(grep -n '# Detect OS platform architecture for pre-built binaries' "$INSTALL_SCRIPT" | head -1 | cut -d: -f1)
  end=$(grep -n 'CALLISTO_BIN_DIR" >> \$GITHUB_PATH' "$INSTALL_SCRIPT" | head -1 | cut -d: -f1)
  end=$((end - 1))
  sed -n "${start},${end}p" "$INSTALL_SCRIPT"
}

run_case() {
  local version="$1"
  local curl_behavior="$2"    # "fail" or "succeed"
  local cargo_behavior="$3"   # "crates-io-succeeds" or "crates-io-fails"
  local verification="${4:-fallback}"
  local arch="${5:-x86_64}"
  local calls_file tmp_script workdir
  calls_file="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  workdir="$(mktemp -d "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  [[ -n "$workdir" && -d "$workdir" ]] || { echo "mktemp -d failed" >&2; exit 1; }
  tmp_script="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  {
    echo "curl() { echo \"curl \$*\" >> '${calls_file}'; [[ '${curl_behavior}' == fail ]] && return 1 || return 0; }"
    echo "tar() { return 0; }"
    echo "unzip() { return 99; }"
    echo "git() { return 99; }"
    echo "gh() { echo \"gh \$*\" >> '${calls_file}'; return 1; }"
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
    echo "uname() { [[ \"\$1\" == -s ]] && echo Linux || echo '${arch}'; }"
    echo "RUNNER_TEMP='${workdir}'"
    echo "GITHUB_WORKSPACE='${workdir}/not-callisto'"
    echo "mkdir -p \"\$GITHUB_WORKSPACE\""
    echo "GITHUB_PATH='${workdir}/github_path'"
    echo "touch \"\$GITHUB_PATH\""
    echo "TAG_NAME='${version}'"
    echo "VERIFICATION='${verification}'"
    echo "SOURCE=auto"
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

# When crates.io install also fails, `latest` fails like a pin does: building the
# default branch would run unreleased code in a job that holds publish credentials.
out=$(run_case "latest" "fail" "crates-io-fails"); code=$?
if [[ $code -eq 0 || "$out" == *"--git"* ]]; then
  echo "FAIL: latest must fail without a git install when crates.io fails too: code=$code out=$out"; fail=1
else
  echo "PASS: latest fails without a git install when crates.io fails too"
fi

for version in callisto@0.5.0 callisto-cli@0.5.0 0.5.0; do
  out=$(run_case "$version" fail crates-io-fails); code=$?
  if [[ $code -eq 0 || "$out" != *"--version 0.5.0"* || "$out" == *"--git"* ]]; then
    echo "FAIL: unavailable pin $version must fail without git fallback: code=$code out=$out"; fail=1
  else
    echo "PASS: unavailable pin $version fails without git fallback"
  fi
done

# verification=require never falls back to crates.io: a missing asset, an unsupported platform or a failed
# attestation must fail and name the asset, with no cargo install attempted.
out=$(run_case "callisto@0.5.0" fail crates-io-succeeds require); code=$?
if [[ $code -eq 0 || "$out" == *"cargo install"* || "$out" != *"callisto-x86_64-unknown-linux-gnu.tar.gz"* ]]; then
  echo "FAIL: require with a missing asset must fail naming it without crates.io: code=$code out=$out"; fail=1
else
  echo "PASS: require with a missing asset fails naming it, no crates.io install"
fi

out=$(run_case "latest" succeed crates-io-succeeds require); code=$?
if [[ $code -eq 0 || "$out" == *"cargo install"* ]]; then
  echo "FAIL: require with a failed attestation must fail without crates.io: code=$code out=$out"; fail=1
else
  echo "PASS: require with a failed attestation fails without crates.io"
fi

out=$(run_case "latest" fail crates-io-succeeds require riscv64); code=$?
if [[ $code -eq 0 || "$out" == *"cargo install"* || "$out" != *"no prebuilt callisto asset"* ]]; then
  echo "FAIL: require on a platform with no asset must fail without crates.io: code=$code out=$out"; fail=1
else
  echo "PASS: require on a platform with no asset fails without crates.io"
fi

# The non-require modes keep the crates.io fallback for a missing asset and an unsupported platform.
for mode in fallback skip; do
  out=$(run_case "callisto@0.5.0" fail crates-io-succeeds "$mode"); code=$?
  if [[ $code -ne 0 || "$out" != *"cargo install callisto-cli --version 0.5.0"* ]]; then
    echo "FAIL: verification=$mode must fall back to crates.io for a missing asset: code=$code out=$out"; fail=1
  else
    echo "PASS: verification=$mode falls back to crates.io for a missing asset"
  fi
done
out=$(run_case "latest" fail crates-io-succeeds fallback riscv64); code=$?
if [[ $code -ne 0 || "$out" != *"cargo install callisto-cli --locked"* ]]; then
  echo "FAIL: fallback on a platform with no asset must use crates.io: code=$code out=$out"; fail=1
else
  echo "PASS: fallback on a platform with no asset uses crates.io"
fi

exit $fail
