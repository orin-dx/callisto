#!/usr/bin/env bash
# Exercise the shared release-artifact shell contract without compiling each
# target. CI's runner matrix performs the real builds; this test protects the
# tuple allowlist and command selection on every workflow-contract change.
set -euo pipefail

root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
source_root="$root/source"
bin_dir="$root/bin"
log="$root/calls.log"
mkdir -p "$source_root" "$bin_dir"

write_stub() {
  local name="$1"
  local body="$2"
  printf '%s\n' '#!/usr/bin/env bash' 'set -euo pipefail' "$body" > "$bin_dir/$name"
  chmod +x "$bin_dir/$name"
}

write_stub rustup 'printf "rustup %s\\n" "$*" >> "$CALLISTO_TEST_LOG"'
write_stub cargo '
printf "cargo %s\\n" "$*" >> "$CALLISTO_TEST_LOG"
if [[ "$1" == "rustc" ]]; then
  mkdir -p "$PWD/target/wasm32-wasip1/release"
  printf wasm > "$PWD/target/wasm32-wasip1/release/callisto_moon.wasm"
fi'
write_stub cross '
printf "cross %s\\n" "$*" >> "$CALLISTO_TEST_LOG"
mkdir -p "$PWD/target/x86_64-unknown-linux-musl/release"
printf binary > "$PWD/target/x86_64-unknown-linux-musl/release/callisto"'
write_stub tar '
printf "tar %s\\n" "$*" >> "$CALLISTO_TEST_LOG"
while [[ "$#" -gt 0 ]]; do
  if [[ "$1" == "-czf" ]]; then
    printf archive > "$2"
    exit 0
  fi
  shift
done
exit 1'

mkdir -p "$source_root/target/aarch64-apple-darwin/release" \
  "$source_root/target/x86_64-unknown-linux-gnu/release"
printf binary > "$source_root/target/aarch64-apple-darwin/release/callisto"
printf binary > "$source_root/target/x86_64-unknown-linux-gnu/release/callisto"

run_supported() {
  local kind="$1" target="$2" asset="$3"
  local output="$root/$kind-$target"
  CALLISTO_TEST_LOG="$log" \
    CALLISTO_RELEASE_ARTIFACT_KIND="$kind" \
    CALLISTO_RELEASE_ARTIFACT_TARGET="$target" \
    CALLISTO_RELEASE_ARTIFACT_ASSET="$asset" \
    CALLISTO_RELEASE_SOURCE_ROOT="$source_root" \
    CALLISTO_RELEASE_ARTIFACT_DIR="$output" \
    PATH="$bin_dir:$PATH" \
    bash .github/scripts/build-release-artifact.sh
  test -f "$output/$asset"
}

run_supported cli aarch64-apple-darwin callisto-aarch64-apple-darwin.tar.gz
run_supported cli x86_64-unknown-linux-gnu callisto-x86_64-unknown-linux-gnu.tar.gz
run_supported cross x86_64-unknown-linux-musl callisto-x86_64-unknown-linux-musl.tar.gz
run_supported wasm wasm32-wasip1 callisto-moon.wasm

rg -F 'cargo install cross --locked --version 0.2.5' "$log" > /dev/null
rg -F 'cross build --locked --release -p callisto-cli --target x86_64-unknown-linux-musl' "$log" > /dev/null
rg -F 'cargo rustc --locked --release -p callisto-moon --target wasm32-wasip1' "$log" > /dev/null
rg -F -- '--features pdk --crate-type cdylib' "$log" > /dev/null

if CALLISTO_RELEASE_ARTIFACT_KIND=cli \
  CALLISTO_RELEASE_ARTIFACT_TARGET=wasm32-wasip1 \
  CALLISTO_RELEASE_ARTIFACT_ASSET=wrong.tar.gz \
  CALLISTO_RELEASE_SOURCE_ROOT="$source_root" \
  CALLISTO_RELEASE_ARTIFACT_DIR="$root/invalid" \
  PATH="$bin_dir:$PATH" \
  bash .github/scripts/build-release-artifact.sh; then
  printf 'unsupported release artifact tuple unexpectedly succeeded\n' >&2
  exit 1
fi

printf 'PASS: shared release artifact build script validates all declared tuples\n'
