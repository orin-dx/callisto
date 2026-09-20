#!/usr/bin/env bash
# Regression test for the download/extraction format dispatch in action.yml.
# Extracts the actual run-step body from action.yml by a
# real text anchor -- the "# Detect OS platform architecture" comment through
# the end of the file -- so this test always exercises the file's current
# real logic; it cannot silently drift from it.
#
# The step's very first three lines (CALLISTO_BIN_DIR assignment, mkdir, and
# TAG_NAME="${{ inputs.version || 'latest' }}") are GitHub Actions template
# expressions that only resolve when the workflow runner preprocesses them;
# taken verbatim as bash they are a syntax error ("bad substitution"). This
# test starts extraction just after that line and supplies CALLISTO_BIN_DIR /
# TAG_NAME itself, matching what the runner would have already substituted.
set -u
ACTION_YML="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/action.yml"

extract_snippet() {
  sed -n '/# Detect OS platform architecture for pre-built binaries/,$p' "$ACTION_YML"
}

# Runs the extracted snippet with uname/curl stubbed per test case, and
# tar/unzip/cargo/cp always stubbed to record invocations into calls_file.
# GITHUB_WORKSPACE points at an empty temp dir (no Cargo.toml present, so the
# local-monorepo-build branch is always skipped here) and RUNNER_TEMP /
# CALLISTO_BIN_DIR / GITHUB_PATH point at temp paths.
run_case() {
  local uname_s="$1"
  local uname_m="$2"
  local curl_exit="$3"
  local gh_mode="${4:-ok}"
  local allow="${5:-false}"
  local calls_file workspace runner_temp bin_dir github_path tmp_script
  calls_file="$(mktemp)"
  workspace="$(mktemp -d)"
  runner_temp="$(mktemp -d)"
  bin_dir="$(mktemp -d)"
  github_path="$(mktemp)"
  tmp_script="$(mktemp)"
  local isolated_path
  isolated_path="$(mktemp -d)"
  for t in tr grep; do ln -s "$(command -v "$t")" "$isolated_path/$t"; done
  {
    printf 'uname() {\n'
    printf '  case "$1" in\n'
    printf '    -s) echo %q ;;\n' "$uname_s"
    printf '    -m) echo %q ;;\n' "$uname_m"
    printf '    *) command uname "$@" ;;\n'
    printf '  esac\n'
    printf '}\n'
    printf 'curl() { echo "$*" >> %q; return %q; }\n' "$calls_file" "$curl_exit"
    printf 'tar() { echo "tar $*" >> %q; return 0; }\n' "$calls_file"
    printf 'unzip() { echo "unzip $*" >> %q; return 0; }\n' "$calls_file"
    printf 'cargo() { echo "cargo $*" >> %q; return 0; }\n' "$calls_file"
    printf 'cp() { echo "cp $*" >> %q; return 0; }\n' "$calls_file"
    case "$gh_mode" in
      ok) printf 'gh() { echo "gh $*" >> %q; return 0; }\n' "$calls_file" ;;
      fail) printf 'gh() { echo "gh $*" >> %q; return 1; }\n' "$calls_file" ;;
      missing) printf 'PATH=%q\n' "$isolated_path" ;;
    esac
    printf 'INPUT_CALLISTO_ALLOW_UNVERIFIED=%q\n' "$allow"
    printf 'GITHUB_WORKSPACE=%q\n' "$workspace"
    printf 'RUNNER_TEMP=%q\n' "$runner_temp"
    printf 'CALLISTO_BIN_DIR=%q\n' "$bin_dir"
    printf 'GITHUB_PATH=%q\n' "$github_path"
    printf 'TAG_NAME="latest"\n'
    extract_snippet
  } > "$tmp_script"
  bash "$tmp_script"
  local code=$?
  echo "---CALLS---"
  cat "$calls_file"
  rm -f "$tmp_script" "$calls_file" "$github_path"
  rm -rf "$workspace" "$runner_temp" "$bin_dir" "$isolated_path"
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

# 5. Download failure -> falls through to the pre-existing cargo-install
# fallback chain untouched: neither tar nor unzip invoked, cargo install
# callisto-cli invoked, step still exits 0. Proves the new dispatch (which
# now runs unconditionally before the if/elif chain) has no side effect on
# that fallback.
out=$(run_case "Linux" "x86_64" 1); code=$?
if [[ $code -ne 0 ]] \
  || [[ "$out" == *"tar -xzf"* ]] \
  || [[ "$out" == *"unzip"* ]] \
  || [[ "$out" != *"cargo install callisto-cli"* ]]; then
  echo "FAIL test_download_failure_falls_back_to_cargo_install: code=$code out=$out"; fail=1
else
  echo "PASS test_download_failure_falls_back_to_cargo_install"
fi

# 6. Verification success -> gh attestation verify runs against the pinned
# repo and release workflow, before extraction, and only the binary is extracted.
out=$(run_case "Linux" "x86_64" 0 ok); code=$?
if [[ $code -ne 0 ]] \
  || [[ "$out" != *"gh attestation verify "*"--repo orin-dx/callisto --signer-workflow orin-dx/callisto/.github/workflows/callisto-release.yml"* ]] \
  || [[ "$out" != *"tar -xzf "*" callisto" ]] \
  || [[ "${out%%tar -xzf*}" != *"gh attestation verify"* ]]; then
  echo "FAIL test_verification_success_extracts_only_binary: code=$code out=$out"; fail=1
else
  echo "PASS test_verification_success_extracts_only_binary"
fi

# 7. Verification failure aborts: nonzero exit, nothing extracted, no fallback install.
out=$(run_case "Linux" "x86_64" 0 fail); code=$?
if [[ $code -eq 0 ]] || [[ "$out" == *"tar -xzf"* ]] || [[ "$out" == *"cargo install"* ]]; then
  echo "FAIL test_verification_failure_aborts: code=$code out=$out"; fail=1
else
  echo "PASS test_verification_failure_aborts"
fi

# 8. gh missing aborts fail-closed.
out=$(run_case "Linux" "x86_64" 0 missing); code=$?
if [[ $code -eq 0 ]] || [[ "$out" == *"tar -xzf"* ]] || [[ "$out" == *"cargo install"* ]]; then
  echo "FAIL test_gh_missing_aborts: code=$code out=$out"; fail=1
else
  echo "PASS test_gh_missing_aborts"
fi

# 9. allow-unverified=true bypasses verification (even with failing/missing gh).
for mode in fail missing; do
  out=$(run_case "Linux" "x86_64" 0 "$mode" true); code=$?
  if [[ $code -ne 0 ]] || [[ "$out" != *"tar -xzf "* ]] || [[ "$out" == *"gh attestation"* ]]; then
    echo "FAIL test_allow_unverified_bypasses_$mode: code=$code out=$out"; fail=1
  else
    echo "PASS test_allow_unverified_bypasses_$mode"
  fi
done

exit $fail
