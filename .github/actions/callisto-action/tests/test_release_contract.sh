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

exit "$fail"
