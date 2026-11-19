#!/usr/bin/env bash
# Runs `callisto release` (mode: release) or `callisto release execute` on a handoff directory (mode: execute) and
# fills published/publishedPackages from its receipt; a rerun adopts what already landed, so a redundant run is a no-op.
set -euo pipefail

cd "$INPUT_CWD"

runtime_dir="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/callisto-release.XXXXXX")"
receipt="$runtime_dir/release-receipt.json"
cleanup() {
  rm -f "$runtime_dir/npmrc" "$receipt"
  rmdir "$runtime_dir" 2>/dev/null || true
}
trap cleanup EXIT

if [[ -n "${NPM_TOKEN:-}" ]]; then
  npm_config="${NPM_CONFIG_USERCONFIG:-${HOME:-}/.npmrc}"
  if [[ -n "${HOME:-}" && -f "$npm_config" ]]; then
    cp "$npm_config" "$runtime_dir/npmrc"
  else
    : > "$runtime_dir/npmrc"
  fi
  chmod 600 "$runtime_dir/npmrc"
  printf '\n//registry.npmjs.org/:_authToken=${NPM_TOKEN}\n' >> "$runtime_dir/npmrc"
  export CALLISTO_PUBLISH_NPM_CONFIG="$runtime_dir/npmrc"
fi

if [[ -n "${PYPI_TOKEN:-}" ]]; then
  export TWINE_USERNAME=__token__
  export TWINE_PASSWORD="$PYPI_TOKEN"
  unset PYPI_TOKEN
fi

if [[ "${INPUT_MODE:-release}" == execute ]]; then
  # The handoff holds the intent, the files a credential-free job packaged, and their manifest when it sealed any.
  handoff="${INPUT_HANDOFF:-}"
  if [[ -z "$handoff" ]]; then
    echo "::error::callisto-action mode 'execute' needs the handoff input"
    exit 1
  fi
  args=(--intent "$handoff/release-intent.json" --receipt "$receipt" --orchestration-revision "$GITHUB_SHA")
  if [[ -f "$handoff/release-manifest.json" ]]; then
    args+=(--artifact-manifest "$handoff/release-manifest.json" --artifact-dir "$handoff")
  fi
  callisto release execute "${args[@]}"
else
  output=$(callisto --format json release --receipt "$receipt")
  if jq -e '.nothingToRelease == true' <<< "$output" > /dev/null; then
    echo 'published=false' >> "$GITHUB_OUTPUT"
    echo 'publishedPackages=[]' >> "$GITHUB_OUTPUT"
    echo '::notice::Nothing to release.'
    exit 0
  fi
fi

packages=$(jq -c '[
    .outcomes[]
    | select(.outcome.kind == "published" and (.operation.role.kind == "registryPublish" or .operation.role.kind == "platformPublish"))
    | .operation.package
  ] | unique' "$receipt")
published=$(jq -r 'length > 0' <<< "$packages")
{
  echo "published=$published"
  echo "publishedPackages=$packages"
} >> "$GITHUB_OUTPUT"
