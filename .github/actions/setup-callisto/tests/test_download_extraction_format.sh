#!/usr/bin/env bash
# Tests the release-asset install in scripts/install-callisto.sh, extracted from the "# Detect OS" comment to EOF.
# Version resolution is covered separately; these fixtures supply the resolved tag.
set -u
ACTION_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_SCRIPT="$ACTION_DIR/scripts/install-callisto.sh"
DOCS_URL="https://github.com/orin-dx/callisto/blob/main/docs/install-in-ci.md"

extract_snippet() {
  sed -n '/^# Detect OS platform architecture for pre-built binaries$/,$p' "$INSTALL_SCRIPT"
}

# Runs the snippet with uname/curl stubbed per case and tar/cargo/cp recording into calls_file;
# GITHUB_WORKSPACE is an empty temp dir so the local-checkout branch is skipped.
run_case() {
  local uname_s="$1" uname_m="$2" curl_exit="$3" gh_mode="${4:-ok}" tag="${5:-latest}"
  local calls_file workspace runner_temp bin_dir github_path tmp_script isolated_path
  calls_file="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  workspace="$(mktemp -d "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  runner_temp="$(mktemp -d "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  bin_dir="$(mktemp -d "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  github_path="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  tmp_script="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  isolated_path="$(mktemp -d "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  for d in "$workspace" "$runner_temp" "$bin_dir" "$isolated_path"; do
    [[ -n "$d" && -d "$d" ]] || { echo "mktemp -d failed" >&2; exit 1; }
  done
  for t in tr grep rm; do ln -s "$(command -v "$t")" "$isolated_path/$t"; done
  {
    printf 'uname() {\n  case "$1" in\n    -s) echo %q ;;\n    -m) echo %q ;;\n    *) return 99 ;;\n  esac\n}\n' "$uname_s" "$uname_m"
    printf 'curl() { echo "curl $*" >> %q; [[ %q -eq 0 ]] && : > "${@: -1}"; return %q; }\n' "$calls_file" "$curl_exit" "$curl_exit"
    printf 'tar() { echo "tar $*" >> %q; return 0; }\n' "$calls_file"
    printf 'cargo() { echo "cargo $*" >> %q; return 0; }\n' "$calls_file"
    printf 'cp() { echo "cp $*" >> %q; return 0; }\n' "$calls_file"
    printf 'git() { echo "unexpected git $*" >> %q; return 99; }\n' "$calls_file"
    case "$gh_mode" in
      # Rejects a flag the real `gh attestation verify` lacks (gh 2.103.3), so a wrong flag fails here.
      ok) printf 'gh() { echo "gh $*" >> %q; local a; for a in "$@"; do [[ "$a" != --* ]] || [[ " %s " == *" $a "* ]] || { echo "gh: unknown flag $a" >&2; return 1; }; done; return 0; }\n' \
        "$calls_file" "--bundle --bundle-from-oci --cert-identity --cert-identity-regex --cert-oidc-issuer --custom-trusted-root --deny-self-hosted-runners --digest-alg --format --hostname --jq --limit --no-public-good --owner --predicate-type --repo --signer-digest --signer-repo --signer-workflow --source-digest --source-ref --template" ;;
      fail) printf 'gh() { echo "gh $*" >> %q; return 1; }\n' "$calls_file" ;;
      missing) printf 'PATH=%q\n' "$isolated_path" ;;
    esac
    printf 'GITHUB_WORKSPACE=%q\nRUNNER_TEMP=%q\nCALLISTO_BIN_DIR=%q\nGITHUB_PATH=%q\n' "$workspace" "$runner_temp" "$bin_dir" "$github_path"
    printf 'TAG_NAME=%q\nSOURCE=auto\nDOCS_URL=%q\n' "$tag" "$DOCS_URL"
    extract_snippet
  } > "$tmp_script"
  bash "$tmp_script" 2>&1
  local code=$?
  [[ -e "$runner_temp/callisto.tar.gz" ]] && echo "ASSET_PRESENT"
  echo "---CALLS---"
  cat "$calls_file"
  rm -f "$tmp_script" "$calls_file" "$github_path"
  rm -rf "$workspace" "$runner_temp" "$bin_dir" "$isolated_path"
  return $code
}

fail=0
check() { # name, condition result (0 = pass), output
  if [[ $2 -eq 0 ]]; then echo "PASS $1"; else echo "FAIL $1: $3"; fail=1; fi
}

# Every supported platform downloads its own asset, verifies it, then extracts only the binary.
for platform in "Darwin arm64 callisto-aarch64-apple-darwin.tar.gz" \
  "Darwin x86_64 callisto-x86_64-apple-darwin.tar.gz" \
  "Linux x86_64 callisto-x86_64-unknown-linux-gnu.tar.gz" \
  "Linux aarch64 callisto-aarch64-unknown-linux-gnu.tar.gz" \
  "Linux arm64 callisto-aarch64-unknown-linux-gnu.tar.gz"; do
  read -r os arch asset <<< "$platform"
  out=$(run_case "$os" "$arch" 0); code=$?
  [[ $code -eq 0 && "$out" == *"curl "*"--retry "*"/releases/latest/download/$asset "* \
    && "$out" == *"gh attestation verify "*"--repo orin-dx/callisto --signer-workflow orin-dx/callisto/.github/workflows/callisto-release.yml"* \
    && "$out" == *"--deny-self-hosted-runners --source-ref refs/heads/main"* \
    && "$out" == *"tar -xzf "*" callisto" && "${out%%tar -xzf*}" == *"gh attestation verify"* \
    && "$out" != *"cargo install"* ]]
  check "test_${os}_${arch}_downloads_verifies_then_extracts_$asset" $? "code=$code out=$out"
done

out=$(run_case Linux x86_64 0 ok "callisto@0.5.0"); code=$?
[[ $code -eq 0 && "$out" == *"/releases/download/callisto@0.5.0/callisto-x86_64-unknown-linux-gnu.tar.gz "* ]]
check test_pinned_tag_downloads_that_release $? "code=$code out=$out"

# Message 1: no prebuilt asset for this platform; nothing is downloaded and crates.io is never used.
for platform in "MINGW64_NT-10.0 x86_64" "Linux riscv64" "SunOS sun4u"; do
  read -r os arch <<< "$platform"
  out=$(run_case "$os" "$arch" 0); code=$?
  [[ $code -ne 0 && "$out" == *"::error::no prebuilt callisto asset exists for "*"$DOCS_URL"* \
    && "$out" != *"curl "* && "$out" != *"tar -xzf"* && "$out" != *"cargo install"* ]]
  check "test_no_asset_for_${os}_${arch}_fails_with_docs_link" $? "code=$code out=$out"
done

# Message 1 also covers a release that has no such asset (download fails); nothing is extracted or installed from crates.io.
out=$(run_case Linux x86_64 22); code=$?
[[ $code -ne 0 && "$out" == *"::error::no prebuilt callisto asset "*"could be downloaded"*"$DOCS_URL"* \
  && "$out" != *"gh attestation"* && "$out" != *"tar -xzf"* && "$out" != *"cargo install"* && "$out" != *"ASSET_PRESENT"* ]]
check test_missing_release_asset_fails_with_docs_link $? "code=$code out=$out"

# Message 2: a failed attestation deletes the archive and never extracts it.
out=$(run_case Linux x86_64 0 fail); code=$?
[[ $code -ne 0 && "$out" == *"::error::attestation for "*"did not verify; do not use this file"*"$DOCS_URL"* \
  && "$out" != *"tar -xzf"* && "$out" != *"cargo install"* && "$out" != *"ASSET_PRESENT"* ]]
check test_failed_attestation_fails_and_discards_archive $? "code=$code out=$out"

# Message 3: without gh nothing is downloaded or extracted.
out=$(run_case Linux x86_64 0 missing); code=$?
[[ $code -ne 0 && "$out" == *"::error::gh is not available"*"$DOCS_URL"* \
  && "$out" != *"curl "* && "$out" != *"tar -xzf"* && "$out" != *"cargo install"* ]]
check test_missing_gh_fails_with_docs_link $? "code=$code out=$out"

# The removed verification input is ignored: setting it can no longer skip verification.
out=$(INPUT_CALLISTO_VERIFICATION=skip run_case Linux x86_64 0 fail); code=$?
[[ $code -ne 0 && "$out" == *"gh attestation verify"* && "$out" != *"tar -xzf"* ]]
check test_verification_input_cannot_skip_verification $? "code=$code out=$out"

exit $fail
