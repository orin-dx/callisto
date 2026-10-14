#!/usr/bin/env bash
# Runs `callisto release` (mode: release) and fills published/publishedPackages
# from its receipt; callisto release's own idempotency makes a redundant run a no-op.
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

output=$(callisto --format json release --receipt "$receipt")

if jq -e '.nothingToRelease == true' <<< "$output" > /dev/null; then
  echo 'published=false' >> "$GITHUB_OUTPUT"
  echo 'publishedPackages=[]' >> "$GITHUB_OUTPUT"
  echo '::notice::Nothing to release.'
  exit 0
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
