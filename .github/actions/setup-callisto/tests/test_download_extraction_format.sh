#!/usr/bin/env bash
# Tests the download/extraction format dispatch in scripts/install-callisto.sh, extracted from the platform-detection comment to EOF.
# Version resolution is covered separately; these fixtures supply the resolved tag.
set -u
ACTION_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_SCRIPT="$ACTION_DIR/scripts/install-callisto.sh"

extract_snippet() {
  sed -n '/# Detect OS platform architecture/,$p' "$INSTALL_SCRIPT"
}

# Runs the snippet with uname/curl stubbed per case and tar/unzip/cargo/cp recording into calls_file;
# GITHUB_WORKSPACE is an empty temp dir so the local-monorepo build branch is skipped.
run_case() {
  local uname_s="$1"
  local uname_m="$2"
  local curl_exit="$3"
  local tag="${4:-latest}"
  local calls_file workspace runner_temp bin_dir github_path tmp_script
  calls_file="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  workspace="$(mktemp -d "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  runner_temp="$(mktemp -d "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  bin_dir="$(mktemp -d "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  github_path="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  tmp_script="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  for d in "$workspace" "$runner_temp" "$bin_dir"; do
    [[ -n "$d" && -d "$d" ]] || { echo "mktemp -d failed" >&2; exit 1; }
  done
  {
    printf 'uname() {\n'
    printf '  case "$1" in\n'
    printf '    -s) echo %q ;;\n' "$uname_s"
    printf '    -m) echo %q ;;\n' "$uname_m"
    printf '    *) return 99 ;;\n'
    printf '  esac\n'
    printf '}\n'
    printf 'curl() { echo "curl $*" >> %q; [[ %q -eq 0 ]] && : > "${@: -1}"; return %q; }\n' "$calls_file" "$curl_exit" "$curl_exit"
    printf 'tar() { echo "tar $*" >> %q; return 0; }\n' "$calls_file"
    printf 'unzip() { echo "unzip $*" >> %q; return 0; }\n' "$calls_file"
    printf 'cargo() { echo "cargo $*" >> %q; return 0; }\n' "$calls_file"
    printf 'cp() { echo "cp $*" >> %q; return 0; }\n' "$calls_file"
    printf 'git() { echo "unexpected git $*" >> %q; return 99; }\n' "$calls_file"
    printf 'GITHUB_WORKSPACE=%q\n' "$workspace"
    printf 'RUNNER_TEMP=%q\n' "$runner_temp"
    printf 'CALLISTO_BIN_DIR=%q\n' "$bin_dir"
    printf 'GITHUB_PATH=%q\n' "$github_path"
    printf 'TAG_NAME=%q\n' "$tag"
    printf 'SOURCE=auto\n'
    extract_snippet
  } > "$tmp_script"
  bash "$tmp_script"
  local code=$?
  [[ -e "$runner_temp/callisto.tar.gz" ]] && echo "ASSET_PRESENT"
  echo "---CALLS---"
  cat "$calls_file"
  rm -f "$tmp_script" "$calls_file" "$github_path"
  rm -rf "$workspace" "$runner_temp" "$bin_dir"
  return $code
}

fail=0

# 1. Windows is not a supported product-binary target. It must skip a
# fictitious download and use the established installation fallback.
out=$(run_case "MINGW64_NT-10.0" "x86_64" 0); code=$?
if [[ $code -ne 0 ]] \
  || [[ "$out" == *"tar -xzf"* ]] \
  || [[ "$out" == *"unzip"* ]] \
  || [[ "$out" != *"cargo install callisto-cli"* ]]; then
  echo "FAIL test_windows_falls_back_without_fictitious_asset: code=$code out=$out"; fail=1
else
  echo "PASS test_windows_falls_back_without_fictitious_asset"
fi

# 2. macOS arm64 regression -> unchanged tar.gz behavior, unzip not invoked.
out=$(run_case "Darwin" "arm64" 0); code=$?
if [[ $code -ne 0 ]] \
  || [[ "$out" != *"tar -xzf "* ]] \
  || [[ "$out" != *" -C "* ]] \
  || [[ "$out" == *"unzip"* ]]; then
  echo "FAIL test_macos_arm64_still_uses_targz: code=$code out=$out"; fail=1
else
  echo "PASS test_macos_arm64_still_uses_targz"
fi

# 3. Linux amd64 regression -> unchanged tar.gz behavior, unzip not invoked.
out=$(run_case "Linux" "x86_64" 0); code=$?
if [[ $code -ne 0 ]] \
  || [[ "$out" != *"tar -xzf "* ]] \
  || [[ "$out" != *" -C "* ]] \
  || [[ "$out" == *"unzip"* ]]; then
  echo "FAIL test_linux_amd64_still_uses_targz: code=$code out=$out"; fail=1
else
  echo "PASS test_linux_amd64_still_uses_targz"
fi

# 3b. Every supported platform downloads its own asset; an older release without it falls back to crates.io (case 5).
for platform in "Darwin arm64 callisto-aarch64-apple-darwin.tar.gz" \
  "Darwin x86_64 callisto-x86_64-apple-darwin.tar.gz" \
  "Linux x86_64 callisto-x86_64-unknown-linux-gnu.tar.gz" \
  "Linux aarch64 callisto-aarch64-unknown-linux-gnu.tar.gz" \
  "Linux arm64 callisto-aarch64-unknown-linux-gnu.tar.gz"; do
  read -r os arch asset <<< "$platform"
  out=$(run_case "$os" "$arch" 0); code=$?
  if [[ $code -ne 0 ]] || [[ "$out" != *"curl "*"/$asset "* ]] || [[ "$out" != *"tar -xzf "* ]]; then
    echo "FAIL test_${os}_${arch}_downloads_$asset: code=$code out=$out"; fail=1
  else
    echo "PASS test_${os}_${arch}_downloads_$asset"
  fi
done
out=$(run_case "Linux" "riscv64" 0); code=$?
if [[ $code -ne 0 ]] || [[ "$out" == *"tar -xzf"* ]] || [[ "$out" != *"cargo install callisto-cli"* ]]; then
  echo "FAIL test_unsupported_architecture_uses_install_fallback: code=$code out=$out"; fail=1
else
  echo "PASS test_unsupported_architecture_uses_install_fallback"
fi

# 4. Unknown/exotic OS must likewise use the fallback chain rather than
# guessing an archive target.
out=$(run_case "SunOS" "sun4u" 0); code=$?
if [[ $code -ne 0 ]] \
  || [[ "$out" == *"tar -xzf "* ]] \
  || [[ "$out" == *"unzip"* ]] \
  || [[ "$out" != *"cargo install callisto-cli"* ]]; then
  echo "FAIL test_unknown_os_uses_install_fallback: code=$code out=$out"; fail=1
else
  echo "PASS test_unknown_os_uses_install_fallback"
fi

# 5. Download failure falls through to the cargo-install chain: no tar or unzip, `cargo install callisto-cli` runs, exit 0.
out=$(run_case "Linux" "x86_64" 1); code=$?
if [[ $code -ne 0 ]] \
  || [[ "$out" == *"tar -xzf"* ]] \
  || [[ "$out" == *"unzip"* ]] \
  || [[ "$out" != *"cargo install callisto-cli"* ]]; then
  echo "FAIL test_download_failure_falls_back_to_cargo_install: code=$code out=$out"; fail=1
else
  echo "PASS test_download_failure_falls_back_to_cargo_install"
fi

exit $fail
