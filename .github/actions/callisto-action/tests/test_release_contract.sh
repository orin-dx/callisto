#!/usr/bin/env bash
# Black-box coverage for callisto-action's mode validation and release-mode
# script: a stubbed `callisto` proves the exact outputs derived from a
# release receipt, without a live GitHub Actions runner. Rust tests cover
# `callisto release`'s own receipt shape.
set -euo pipefail

action_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mode_script="$action_dir/scripts/validate-mode.sh"
release_script="$action_dir/scripts/run-release.sh"

fail=0

# --- validate-mode.sh: accepted values and rejection ---
for mode in version-pr release; do
  if ! INPUT_MODE="$mode" bash "$mode_script" > /dev/null 2>&1; then
    echo "FAIL: mode '$mode' must be accepted"
    fail=1
  fi
done
echo 'PASS: validate-mode accepts version-pr and release'

set +e
invalid_output=$(INPUT_MODE=bogus bash "$mode_script" 2>&1)
invalid_rc=$?
set -e
if [[ "$invalid_rc" == 0 ]] \
  || [[ "$invalid_output" != *"'version-pr' or 'release'"* ]] \
  || [[ "$invalid_output" != *"'bogus'"* ]]; then
  echo "FAIL: an invalid mode must fail naming the accepted values: $invalid_output"
  fail=1
else
  echo 'PASS: validate-mode rejects an unsupported mode, naming the accepted values'
fi

# Runs run-release.sh against a stubbed `callisto` that prints $1 to stdout
# and, when --receipt PATH is given, also writes $1 to PATH.
run_release() {
  local receipt_json="$1" workdir output_file rc
  workdir="$(mktemp -d)"
  output_file="$(mktemp)"
  cat > "$workdir/callisto" <<STUB
#!/usr/bin/env bash
prev=''
out=''
for a in "\$@"; do
  [[ "\$prev" == '--receipt' ]] && out="\$a"
  prev="\$a"
done
printf '%s' '$receipt_json'
[[ -n "\$out" ]] && printf '%s' '$receipt_json' > "\$out"
STUB
  chmod +x "$workdir/callisto"
  set +e
  PATH="$workdir:$PATH" INPUT_CWD=. GITHUB_OUTPUT="$output_file" bash "$release_script" > /dev/null 2>&1
  rc=$?
  set -e
  cat "$output_file"
  rm -rf "$workdir" "$output_file"
  return "$rc"
}

# Nothing to release keeps the fixed false/[] values.
output="$(run_release '{"nothingToRelease":true}')"
if [[ "$output" != *'published=false'* ]] || [[ "$output" != *'publishedPackages=[]'* ]]; then
  echo "FAIL: nothing-to-release must keep published=false/publishedPackages=[]: $output"
  fail=1
else
  echo 'PASS: run-release keeps published=false/publishedPackages=[] when nothing to release'
fi

# A receipt with published registry/platform outcomes fills both
# outputs; a published `tag` outcome and an `alreadySatisfied` one are excluded.
receipt='{"outcomes":[
  {"operation":{"package":"cargo/app","role":{"kind":"registryPublish"},"version":"1.0.0"},"outcome":{"kind":"published"}},
  {"operation":{"package":"cargo/lib","role":{"kind":"registryPublish"},"version":"1.0.0"},"outcome":{"kind":"alreadySatisfied"}},
  {"operation":{"package":"cargo/app","role":{"kind":"tag"},"version":"1.0.0"},"outcome":{"kind":"published"}}
]}'
output="$(run_release "$receipt")"
if [[ "$output" != *'published=true'* ]] || [[ "$output" != *'publishedPackages=["cargo/app"]'* ]]; then
  echo "FAIL: a published registry outcome must fill published/publishedPackages: $output"
  fail=1
else
  echo 'PASS: run-release fills published/publishedPackages from published registry/platform outcomes only'
fi

# The action passes publisher inputs to Callisto without mapping native tool auth process-wide.
auth_dir="$(mktemp -d)"
auth_home="$auth_dir/home"
auth_runner_temp="$auth_dir/runner-temp"
auth_bin="$auth_dir/bin"
mkdir -p "$auth_home" "$auth_runner_temp" "$auth_bin"
printf 'legacy=true\n//registry.example/:_authToken=${NPM_TOKEN}\n' > "$auth_home/.npmrc"
cat > "$auth_bin/callisto" <<'STUB'
#!/usr/bin/env bash
[[ "${NPM_TOKEN:-}" == 'secret-npm-value' && -z "${PYPI_TOKEN:-}" ]] || exit 21
[[ "${TWINE_USERNAME:-}" == '__token__' && "${TWINE_PASSWORD:-}" == 'secret-pypi-value' ]] || exit 25
[[ -z "${NPM_CONFIG_USERCONFIG:-}" && -z "${NPM_CONFIG_TOKEN:-}" ]] || exit 26
[[ "$(stat -f '%Lp' "$CALLISTO_PUBLISH_NPM_CONFIG" 2>/dev/null || stat -c '%a' "$CALLISTO_PUBLISH_NPM_CONFIG")" == 600 ]] || exit 22
grep -Fxq 'legacy=true' "$CALLISTO_PUBLISH_NPM_CONFIG" || exit 23
grep -Fxq '//registry.example/:_authToken=${NPM_TOKEN}' "$CALLISTO_PUBLISH_NPM_CONFIG" || exit 30
grep -Fxq '//registry.npmjs.org/:_authToken=${NPM_TOKEN}' "$CALLISTO_PUBLISH_NPM_CONFIG" || exit 24
printf '%s' "$CALLISTO_PUBLISH_NPM_CONFIG" > "$AUTH_CONFIG_CAPTURE"
printf '%s' '{"outcomes":[]}'
prev=''
for a in "$@"; do
  [[ "$prev" == '--receipt' ]] && printf '%s' '{"outcomes":[]}' > "$a"
  prev="$a"
done
STUB
chmod +x "$auth_bin/callisto"
auth_output="$auth_dir/github-output"
if ! PATH="$auth_bin:$PATH" HOME="$auth_home" RUNNER_TEMP="$auth_runner_temp" \
  AUTH_CONFIG_CAPTURE="$auth_dir/auth-config-path" INPUT_CWD=. GITHUB_OUTPUT="$auth_output" \
  NPM_TOKEN=secret-npm-value PYPI_TOKEN=secret-pypi-value bash "$release_script" > /dev/null 2>&1; then
  echo 'FAIL: run-release must pass unmapped publisher inputs to Callisto'
  fail=1
else
  auth_config="$(<"$auth_dir/auth-config-path")"
  if [[ -e "$auth_config" ]] || [[ "$(<"$auth_home/.npmrc")" != $'legacy=true\n//registry.example/:_authToken=${NPM_TOKEN}' ]]; then
    echo 'FAIL: run-release must remove its temporary npm config and preserve the user config'
    fail=1
  else
    echo 'PASS: run-release prepares publisher inputs and removes temporary npm config'
  fi
fi

# A Bun workspace must not cause NPM_CONFIG_TOKEN to be set on the whole release process.
bun_workspace="$auth_dir/bun-workspace"
mkdir -p "$bun_workspace"
: > "$bun_workspace/bun.lock"
if ! PATH="$auth_bin:$PATH" HOME="$auth_home" RUNNER_TEMP="$auth_runner_temp" \
  AUTH_CONFIG_CAPTURE="$auth_dir/bun-auth-config-path" INPUT_CWD="$bun_workspace" GITHUB_OUTPUT="$auth_output" \
  NPM_TOKEN=secret-npm-value PYPI_TOKEN=secret-pypi-value TEST_BUN_PUBLISH=true \
  bash "$release_script" > /dev/null 2>&1; then
  echo 'FAIL: run-release must leave Bun token mapping to publisher dispatch'
  fail=1
else
  echo 'PASS: run-release leaves Bun token mapping to publisher dispatch'
fi
rm -rf "$auth_dir"

# Callisto-action's own environment setup must not use `$/` or an
# external owner/repo/path@ref for its sibling setup-callisto -- both resolve
# against whatever ref invoked *this* action, which may not exist verbatim in
# setup-callisto's own history. It must call the sibling script directly by
# github.action_path instead, so it always uses its own checked-out ref.
action_contents="$(<"$action_dir/action.yml")"
if [[ "$action_contents" == *'uses: $/.github/actions/setup-callisto'* ]] \
  || [[ "$action_contents" == *'uses: orin-dx/callisto/.github/actions/setup-callisto@'* ]]; then
  echo 'FAIL: callisto-action must not reference setup-callisto via `uses:` at all (ref-ambiguous)'
  fail=1
elif [[ "$action_contents" != *'"${{ github.action_path }}/../setup-callisto/scripts/install-callisto.sh"'* ]]; then
  echo 'FAIL: callisto-action must call setup-callisto/scripts/install-callisto.sh by github.action_path'
  fail=1
elif [[ ! -f "$action_dir/../setup-callisto/scripts/install-callisto.sh" ]]; then
  echo 'FAIL: the sibling install-callisto.sh script callisto-action calls into does not exist'
  fail=1
else
  echo 'PASS: setup-callisto is called by github.action_path, not a ref-ambiguous uses:'
fi

# The mode input, its default, and both scripts are wired.
if [[ "$action_contents" != *$'mode:\n    description:'* ]] \
  || [[ "$action_contents" != *"default: 'version-pr'"* ]] \
  || [[ "$action_contents" != *'bash "$GITHUB_ACTION_PATH/scripts/validate-mode.sh"'* ]] \
  || [[ "$action_contents" != *'bash "$GITHUB_ACTION_PATH/scripts/run-release.sh"'* ]] \
  || [[ "$action_contents" != *'bash "$GITHUB_ACTION_PATH/scripts/create-or-update-release-pr.sh"'* ]]; then
  echo 'FAIL: action metadata does not wire mode, its default, and both mode scripts'
  fail=1
else
  echo 'PASS: action metadata wires mode (default version-pr) to both scripts'
fi

# Published/publishedPackages read from the release step, falling
# back to the version-pr mode's fixed false/[] when that step did not run.
if [[ "$action_contents" != *"steps.release.outputs.published || 'false'"* ]] \
  || [[ "$action_contents" != *"steps.release.outputs.publishedPackages || '[]'"* ]]; then
  echo 'FAIL: published/publishedPackages must read the release step, falling back to false/[]'
  fail=1
else
  echo 'PASS: published/publishedPackages read the release step, falling back to false/[]'
fi

# callisto-action installs the version it is itself pinned to by default,
# not a hardcoded 'latest', and exposes an explicit override input.
if [[ "$action_contents" != *$'  version:\n'* ]] \
  || [[ "$action_contents" == *'INPUT_CALLISTO_VERSION: latest'* ]] \
  || [[ "$action_contents" != *'INPUT_CALLISTO_VERSION: ${{ inputs.version }}'* ]] \
  || [[ "$action_contents" != *'GITHUB_ACTION_REF: ${{ github.action_ref }}'* ]]; then
  echo 'FAIL: callisto-action must expose a version input and pass it and github.action_ref to install-callisto.sh'
  fail=1
else
  echo 'PASS: callisto-action exposes a version input and passes it and github.action_ref through'
fi

exit "$fail"
