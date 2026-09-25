#!/usr/bin/env bash
# Installs or builds the Callisto binary. Factored out of action.yml so
# callisto-action's own environment setup can call it directly by path
# (github.action_path has no ref-ambiguous `uses:` to resolve), instead of
# duplicating this logic.
set -euo pipefail

# Install into $RUNNER_TEMP so the binary never lands inside the workspace
# tree (preventing accidental git-staging by downstream steps that run
# "git add -A").
CALLISTO_BIN_DIR="${RUNNER_TEMP}/callisto-bin"
mkdir -p "$CALLISTO_BIN_DIR"
TAG_NAME="$INPUT_CALLISTO_VERSION"
SOURCE="$INPUT_CALLISTO_SOURCE"

if [[ "$SOURCE" != "auto" && "$SOURCE" != "local" ]]; then
  echo "::error::unsupported setup-callisto source '$SOURCE' (expected auto or local)"
  exit 1
fi

# Validate verification mode
VERIFICATION="${INPUT_CALLISTO_VERIFICATION:-fallback}"
case "$VERIFICATION" in
  require|fallback|skip) ;;
  *)
    echo "::error::unsupported setup-callisto verification '$VERIFICATION' (valid values: require, fallback, skip)"
    exit 1
    ;;
esac

# Detect OS platform architecture for pre-built binaries
OS_NAME=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH_NAME=$(uname -m)

if [[ "$OS_NAME" == "darwin" ]]; then
  if [[ "$ARCH_NAME" == "arm64" ]]; then
    ASSET_NAME="callisto-aarch64-apple-darwin.tar.gz"
  else
    ASSET_NAME=""
  fi
elif [[ "$OS_NAME" == "linux" ]]; then
  if [[ "$ARCH_NAME" == "x86_64" ]]; then
    ASSET_NAME="callisto-x86_64-unknown-linux-gnu.tar.gz"
  else
    ASSET_NAME=""
  fi
elif [[ "$OS_NAME" == *"mingw"* || "$OS_NAME" == *"msys"* || "$OS_NAME" == *"windows"* ]]; then
  ASSET_NAME=""
else
  ASSET_NAME=""
fi

if [[ -n "$ASSET_NAME" && "$TAG_NAME" == "latest" ]]; then
  DOWNLOAD_URL="https://github.com/orin-dx/callisto/releases/latest/download/$ASSET_NAME"
elif [[ -n "$ASSET_NAME" ]]; then
  DOWNLOAD_URL="https://github.com/orin-dx/callisto/releases/download/$TAG_NAME/$ASSET_NAME"
fi

# Derive a crates.io version spec from TAG_NAME: "latest" installs
# whatever crates.io resolves as latest (no --version flag); an
# explicit tag ("callisto@0.5.0" or legacy "callisto-cli@0.5.0")
# strips either prefix down to the bare semver cargo install expects.
CRATE_VERSION="${TAG_NAME#callisto@}"; CRATE_VERSION="${CRATE_VERSION#callisto-cli@}"

# Pick a download destination whose extension matches ASSET_NAME
# (under $RUNNER_TEMP, never the workspace) so the extraction step
# below always has an archive it can actually open -- a hardcoded
# ".tar.gz" path here previously masked the fact that Windows assets
# are zip files, since tar -xzf can't read a zip regardless of what
# the file on disk is really named.
case "$ASSET_NAME" in
  *.zip)
    DOWNLOAD_DEST="${RUNNER_TEMP}/callisto.zip"
    ;;
  *.tar.gz)
    DOWNLOAD_DEST="${RUNNER_TEMP}/callisto.tar.gz"
    ;;
  *)
    DOWNLOAD_DEST="${RUNNER_TEMP}/callisto.tar.gz"
    ;;
esac

# A prebuilt asset runs with the job's privileges, so it must carry a
# GitHub attestation from the Callisto release workflow before it is
# extracted. require aborts on failure; fallback discards the asset and
# returns 1 so the crates.io path runs; skip trusts the asset.
verify_asset() {
  local reason=""
  if [[ "$VERIFICATION" == "skip" ]]; then
    echo "::warning::verification=skip: $ASSET_NAME was not verified"
    return 0
  fi
  if ! command -v gh >/dev/null 2>&1; then
    reason="gh is not installed"
  elif ! gh attestation verify "$DOWNLOAD_DEST" \
      --repo orin-dx/callisto \
      --signer-workflow orin-dx/callisto/.github/workflows/callisto-release.yml \
      --deny-self-hosted-runners; then
    reason="attestation verification failed"
  fi
  [[ -z "$reason" ]] && return 0
  rm -f "$DOWNLOAD_DEST"
  if [[ "$VERIFICATION" == "require" ]]; then
    echo "::error::cannot verify $ASSET_NAME: $reason (verification=require)"
    exit 1
  fi
  echo "::warning::cannot verify $ASSET_NAME: $reason; discarding it and installing from crates.io (verification=fallback)"
  return 1
}

# Download, verify and extract the prebuilt asset; nonzero means use the fallback chain.
fetch_prebuilt() {
  curl -sL -f "$DOWNLOAD_URL" -o "$DOWNLOAD_DEST" 2>/dev/null || return 1
  verify_asset || return 1
  # Extract only the expected binary, never the whole archive.
  case "$ASSET_NAME" in
    *.zip)
      unzip -q "$DOWNLOAD_DEST" callisto.exe -d "$CALLISTO_BIN_DIR"
      ;;
    *)
      tar -xzf "$DOWNLOAD_DEST" -C "$CALLISTO_BIN_DIR" callisto
      ;;
  esac
}

# Prioritize local repository build when inside the callisto monorepo
if [[ -f "$GITHUB_WORKSPACE/Cargo.toml" ]] && grep -q "callisto-cli" "$GITHUB_WORKSPACE/Cargo.toml"; then
  echo "Building local callisto binary from current repository commit..."
  cargo build --locked -p callisto-cli
  cp "$GITHUB_WORKSPACE/target/debug/callisto" "$CALLISTO_BIN_DIR/callisto"
elif [[ "$SOURCE" == "local" ]]; then
  echo "::error::setup-callisto source=local requires a Callisto workspace checkout"
  exit 1
elif [[ -n "$ASSET_NAME" ]] && fetch_prebuilt; then
  echo "Callisto downloaded from GitHub Releases ($DOWNLOAD_URL)"
elif [[ "$TAG_NAME" == "latest" ]] && cargo install callisto-cli --locked --root "${RUNNER_TEMP}/callisto-cargo-install" --quiet; then
  echo "Callisto installed from crates.io (latest)"
  cp "${RUNNER_TEMP}/callisto-cargo-install/bin/callisto" "$CALLISTO_BIN_DIR/callisto"
elif [[ "$TAG_NAME" != "latest" ]] && cargo install callisto-cli --version "$CRATE_VERSION" --locked --root "${RUNNER_TEMP}/callisto-cargo-install" --quiet; then
  echo "Callisto installed from crates.io ($CRATE_VERSION)"
  cp "${RUNNER_TEMP}/callisto-cargo-install/bin/callisto" "$CALLISTO_BIN_DIR/callisto"
else
  if [[ "$TAG_NAME" != "latest" ]]; then
    echo "::error::callisto $TAG_NAME is available neither as a prebuilt asset nor on crates.io"
    exit 1
  fi
  echo "Installing from git source (crates.io install failed or was skipped)..."
  cargo install --locked --git https://github.com/orin-dx/callisto.git callisto-cli --root "${RUNNER_TEMP}/callisto-cargo-install" --quiet
  cp "${RUNNER_TEMP}/callisto-cargo-install/bin/callisto" "$CALLISTO_BIN_DIR/callisto"
fi

echo "$CALLISTO_BIN_DIR" >> $GITHUB_PATH
if [[ -f "$GITHUB_WORKSPACE/.github/callisto-problem-matcher.json" ]]; then
  echo "::add-matcher::$GITHUB_WORKSPACE/.github/callisto-problem-matcher.json"
fi
