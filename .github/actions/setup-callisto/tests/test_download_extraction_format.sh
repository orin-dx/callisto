#!/usr/bin/env bash
# Regression test for the download/extraction format dispatch in
# scripts/install-callisto.sh. Extracts the actual script body by a real
# text anchor -- the "# Validate verification mode" comment through the end
# of the file -- so this test always exercises the file's current real
# logic; it cannot silently drift from it.
#
# The step's very first three lines (CALLISTO_BIN_DIR assignment, mkdir, and
# TAG_NAME="${{ inputs.version || 'latest' }}") are GitHub Actions template
# expressions that only resolve when the workflow runner preprocesses them;
# taken verbatim as bash they are a syntax error ("bad substitution"). This
# test starts extraction just after that line and supplies CALLISTO_BIN_DIR /
# TAG_NAME itself, matching what the runner would have already substituted.
set -u
ACTION_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ACTION_YML="$ACTION_DIR/action.yml"
INSTALL_SCRIPT="$ACTION_DIR/scripts/install-callisto.sh"

extract_snippet() {
  sed -n '/# Validate verification mode/,$p' "$INSTALL_SCRIPT"
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
  local mode="${5-}"
  local tag="${6:-latest}"
  local calls_file workspace runner_temp bin_dir github_path tmp_script
  calls_file="$(mktemp)"
  workspace="$(mktemp -d)"
  runner_temp="$(mktemp -d)"
  bin_dir="$(mktemp -d)"
  github_path="$(mktemp)"
  tmp_script="$(mktemp)"
  local isolated_path
  isolated_path="$(mktemp -d)"
  for t in tr grep rm; do ln -s "$(command -v "$t")" "$isolated_path/$t"; done
  {
    printf 'uname() {\n'
    printf '  case "$1" in\n'
    printf '    -s) echo %q ;;\n' "$uname_s"
    printf '    -m) echo %q ;;\n' "$uname_m"
    printf '    *) command uname "$@" ;;\n'
    printf '  esac\n'
    printf '}\n'
    printf 'curl() { echo "curl $*" >> %q; [[ %q -eq 0 ]] && : > "${@: -1}"; return %q; }\n' "$calls_file" "$curl_exit" "$curl_exit"
    printf 'tar() { echo "tar $*" >> %q; return 0; }\n' "$calls_file"
    printf 'unzip() { echo "unzip $*" >> %q; return 0; }\n' "$calls_file"
    printf 'cargo() { echo "cargo $*" >> %q; return 0; }\n' "$calls_file"
    printf 'cp() { echo "cp $*" >> %q; return 0; }\n' "$calls_file"
    case "$gh_mode" in
      ok) printf 'gh() { echo "gh $*" >> %q; return 0; }\n' "$calls_file" ;;
      fail) printf 'gh() { echo "gh $*" >> %q; return 1; }\n' "$calls_file" ;;
      missing) printf 'PATH=%q\n' "$isolated_path" ;;
    esac
    [[ -n "$mode" ]] && printf 'INPUT_CALLISTO_VERIFICATION=%q\n' "$mode"
    printf 'GITHUB_WORKSPACE=%q\n' "$workspace"
    printf 'RUNNER_TEMP=%q\n' "$runner_temp"
    printf 'CALLISTO_BIN_DIR=%q\n' "$bin_dir"
    printf 'GITHUB_PATH=%q\n' "$github_path"
    printf 'TAG_NAME=%q\n' "$tag"
    extract_snippet
  } > "$tmp_script"
  bash "$tmp_script"
  local code=$?
  [[ -e "$runner_temp/callisto.tar.gz" ]] && echo "ASSET_PRESENT"
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

# 6. Verification success extracts only the binary in every mode but skip
# (skip never calls gh); gh runs against the pinned repo and signer, before extraction.
for mode in require fallback ""; do
  out=$(run_case "Linux" "x86_64" 0 ok "$mode"); code=$?
  if [[ $code -ne 0 ]] \
    || [[ "$out" != *"gh attestation verify "*"--repo orin-dx/callisto --signer-workflow orin-dx/callisto/.github/workflows/callisto-release.yml"* ]] \
    || [[ "$out" != *"tar -xzf "*" callisto" ]] \
    || [[ "${out%%tar -xzf*}" != *"gh attestation verify"* ]] \
    || [[ "$out" == *"cargo install"* ]]; then
    echo "FAIL test_verification_success_extracts_only_binary_${mode:-default}: code=$code out=$out"; fail=1
  else
    echo "PASS test_verification_success_extracts_only_binary_${mode:-default}"
  fi
done

# 7. require + verification failure or missing gh aborts; asset never extracted or kept.
for gh in fail missing; do
  out=$(run_case "Linux" "x86_64" 0 "$gh" require); code=$?
  if [[ $code -eq 0 ]] || [[ "$out" == *"tar -xzf"* ]] || [[ "$out" == *"cargo install"* ]] \
    || [[ "$out" == *"ASSET_PRESENT"* ]] || [[ "$out" != *"::error::"* ]]; then
    echo "FAIL test_require_$([[ $gh == fail ]] && echo verification_failure || echo gh_missing)_aborts: code=$code out=$out"; fail=1
  else
    echo "PASS test_require_$([[ $gh == fail ]] && echo verification_failure || echo gh_missing)_aborts"
  fi
done

# 8. fallback (explicit and default) + failure or missing gh: warn, delete the asset,
# take crates.io, never extract.
for gh in fail missing; do
  for mode in fallback ""; do
    out=$(run_case "Linux" "x86_64" 0 "$gh" "$mode"); code=$?
    name="test_fallback_$([[ $gh == fail ]] && echo verification_failure || echo gh_missing)_uses_crates_io_${mode:-default}"
    if [[ $code -ne 0 ]] || [[ "$out" == *"tar -xzf"* ]] || [[ "$out" == *"unzip"* ]] \
      || [[ "$out" == *"ASSET_PRESENT"* ]] || [[ "$out" != *"::warning::"* ]] \
      || [[ "$out" != *"cargo install callisto-cli"* ]] || [[ "$out" == *"cargo install --locked --git"* ]]; then
      echo "FAIL $name: code=$code out=$out"; fail=1
    else
      echo "PASS $name"
    fi
  done
done

# 9. fallback keeps an explicit version pin and never uses the unpinned git install.
out=$(run_case "Linux" "x86_64" 0 fail fallback "callisto@0.5.0"); code=$?
if [[ $code -ne 0 ]] || [[ "$out" == *"tar -xzf"* ]] \
  || [[ "$out" != *"cargo install callisto-cli --version 0.5.0 "* ]] || [[ "$out" == *"--git"* ]]; then
  echo "FAIL test_fallback_keeps_explicit_version_pin: code=$code out=$out"; fail=1
else
  echo "PASS test_fallback_keeps_explicit_version_pin"
fi

# 10. skip extracts without calling gh, with a warning.
for gh in fail missing ok; do
  out=$(run_case "Linux" "x86_64" 0 "$gh" skip); code=$?
  if [[ $code -ne 0 ]] || [[ "$out" != *"tar -xzf "* ]] || [[ "$out" == *"gh attestation"* ]] \
    || [[ "$out" != *"::warning::"* ]] || [[ "$out" == *"cargo install"* ]]; then
    echo "FAIL test_skip_extracts_without_gh_$gh: code=$code out=$out"; fail=1
  else
    echo "PASS test_skip_extracts_without_gh_$gh"
  fi
done

# 11. Invalid value fails immediately, listing the valid values, with no download.
out=$(run_case "Linux" "x86_64" 0 ok bogus); code=$?
if [[ $code -eq 0 ]] || [[ "$out" != *"valid values: require, fallback, skip"* ]] \
  || [[ "$out" == *"curl"* ]] || [[ "$out" == *"tar -xzf"* ]]; then
  echo "FAIL test_invalid_verification_fails: code=$code out=$out"; fail=1
else
  echo "PASS test_invalid_verification_fails"
fi

# 12. The action's declared input default is fallback.
if grep -A12 '^  verification:' "$ACTION_YML" | grep -q "default: 'fallback'"; then
  echo "PASS test_default_verification_is_fallback"
else
  echo "FAIL test_default_verification_is_fallback"; fail=1
fi

exit $fail
