#!/usr/bin/env bash
# Runs the download step's run body from action.yml with curl/gh/cargo/cp/rustup stubbed.
set -u
ACTION_YML="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/action.yml"

extract_snippet() {
  awk '/id: download-wasm/{f=1} f&&/run: \|/{r=1;next} r{print}' "$ACTION_YML" | sed 's/^        //'
}

run_case() {
  local gh_mode="$1" mode="${2-}"
  local dir calls script isolated
  dir="$(mktemp -d)"; calls="$(mktemp)"; script="$(mktemp)"; isolated="$(mktemp -d)"
  for t in rm dirname mkdir; do ln -s "$(command -v "$t")" "$isolated/$t"; done
  {
    printf 'curl() { echo "curl $*" >> %q; : > "${@: -1}"; return 0; }\n' "$calls"
    printf 'cargo() { echo "cargo $*" >> %q; }\nrustup() { :; }\ncp() { echo "cp $*" >> %q; }\n' "$calls" "$calls"
    case "$gh_mode" in
      ok) printf 'gh() { echo "gh $*" >> %q; return 0; }\n' "$calls" ;;
      fail) printf 'gh() { echo "gh $*" >> %q; return 1; }\n' "$calls" ;;
      missing) printf 'PATH=%q\n' "$isolated" ;;
    esac
    [[ -n "$mode" ]] && printf 'INPUT_WASM_VERIFICATION=%q\n' "$mode"
    printf 'INPUT_WASM_DEST=%q\nINPUT_WASM_VERSION=latest\nGITHUB_OUTPUT=%q\n' "$dir/out/plugin.wasm" "$dir/gho"
    extract_snippet
  } > "$script"
  bash "$script"; local code=$?
  [[ -e "$dir/out/plugin.wasm" ]] && echo "PLUGIN_PRESENT"
  echo "---CALLS---"; cat "$calls"
  rm -rf "$dir" "$calls" "$script" "$isolated"
  return $code
}

fail=0
check() { # name, condition-result
  if [[ $2 -eq 0 ]]; then echo "PASS $1"; else echo "FAIL $1: $out"; fail=1; fi
}

for mode in require fallback ""; do
  out=$(run_case ok "$mode"); code=$?
  [[ $code -eq 0 && "$out" == *"gh attestation verify"*"--repo orin-dx/callisto"* && "$out" == *PLUGIN_PRESENT* ]] && rc=0 || rc=1
  check "test_wasm_verification_success_${mode:-default}" $rc
done

for gh in fail missing; do
  for mode in require fallback ""; do
    out=$(run_case "$gh" "$mode"); code=$?
    [[ $code -ne 0 && "$out" != *PLUGIN_PRESENT* && "$out" == *"::error::"* && "$out" != *"cargo build"* ]] && rc=0 || rc=1
    check "test_wasm_${mode:-default}_${gh}_aborts" $rc
  done
done

out=$(run_case fail fallback)
[[ "$out" == *"no fallback exists for the wasm plugin"* ]] && rc=0 || rc=1
check "test_wasm_fallback_error_names_no_fallback" $rc

for gh in fail missing ok; do
  out=$(run_case "$gh" skip); code=$?
  [[ $code -eq 0 && "$out" == *PLUGIN_PRESENT* && "$out" == *"::warning::"* && "$out" != *"gh attestation"* ]] && rc=0 || rc=1
  check "test_wasm_skip_uses_plugin_without_gh_$gh" $rc
done

out=$(run_case ok bogus); code=$?
[[ $code -ne 0 && "$out" == *"valid values: require, fallback, skip"* && "$out" != *"curl"* ]] && rc=0 || rc=1
check "test_wasm_invalid_verification_fails" $rc

grep -A12 '^  verification:' "$ACTION_YML" | grep -q "default: 'fallback'" && rc=0 || rc=1
check "test_wasm_default_verification_is_fallback" $rc

exit $fail
