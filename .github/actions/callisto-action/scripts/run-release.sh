#!/usr/bin/env bash
# Runs `callisto release` (mode: release) and fills published/publishedPackages
# from its receipt; callisto release's own idempotency makes a redundant run a no-op.
set -euo pipefail

cd "$INPUT_CWD"

receipt="$(mktemp -d)/release-receipt.json"
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
