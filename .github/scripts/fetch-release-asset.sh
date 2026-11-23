#!/usr/bin/env bash
# Downloads one release asset, verifies its attestation exactly as setup-callisto does, and extracts the binary.
# Usage: fetch-release-asset.sh <release-tag> <asset-name> <dest-dir>; needs GH_TOKEN.
set -euo pipefail

tag=$1
asset=$2
dest=$3

mkdir -p "$dest"
gh release download "$tag" --repo orin-dx/callisto --pattern "$asset" --dir "$dest" --clobber
gh attestation verify "$dest/$asset" \
  --repo orin-dx/callisto \
  --signer-workflow orin-dx/callisto/.github/workflows/callisto-release.yml \
  --deny-self-hosted-runners \
  --source-ref refs/heads/main
tar -xzf "$dest/$asset" -C "$dest" callisto
