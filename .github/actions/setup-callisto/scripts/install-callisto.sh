#!/usr/bin/env bash
# Installs or builds the Callisto binary; callisto-action calls it directly by path.
set -euo pipefail

# Install into $RUNNER_TEMP so the binary never lands inside the workspace
# tree (preventing accidental git-staging by downstream steps that run
# "git add -A").
CALLISTO_BIN_DIR="${RUNNER_TEMP}/callisto-bin"
mkdir -p "$CALLISTO_BIN_DIR"
TAG_NAME="$INPUT_CALLISTO_VERSION"
SOURCE="$INPUT_CALLISTO_SOURCE"
DOCS_URL="https://github.com/orin-dx/callisto/blob/main/docs/install-in-ci.md"

if [[ "$SOURCE" != "auto" && "$SOURCE" != "local" ]]; then
  echo "::error::unsupported setup-callisto source '$SOURCE' (expected auto or local)"
  exit 1
fi

# Without an override, install the release this action's ref is pinned to (`callisto@X.Y.Z`), else the latest;
# a failed tag lookup fails the step, and a named ref such as `v1` stands for its commit.
if [[ "$SOURCE" == "auto" && "$TAG_NAME" == "latest" && -n "${GITHUB_ACTION_REF:-}" ]]; then
  if ! remote_tags=$(git ls-remote --tags https://github.com/orin-dx/callisto.git 'callisto@*' "$GITHUB_ACTION_REF"); then
    echo "::error::could not list callisto release tags to resolve action ref $GITHUB_ACTION_REF; set the version input to install a specific release"
    exit 1
  fi
  ref_commit=$(printf '%s\n' "$remote_tags" \
    | awk -v ref="refs/tags/$GITHUB_ACTION_REF" '$2 == ref || $2 == ref "^{}" { sha = $1 } END { print sha }')
  resolved_tag=$(printf '%s\n' "$remote_tags" \
    | awk -v ref="${ref_commit:-$GITHUB_ACTION_REF}" '$1 == ref && $2 ~ /^refs\/tags\/callisto@/ { print $2 }' \
    | sed -e 's#^refs/tags/##' -e 's/\^{}$//' \
    | sort -V | tail -n1) || true
  if [[ -n "$resolved_tag" ]]; then
    echo "Resolved action ref $GITHUB_ACTION_REF to $resolved_tag"
    TAG_NAME="$resolved_tag"
  fi
fi

# A bare version (0.21.0) stands for its callisto@ tag; any other value must already be one, so the URL never carries free text.
if [[ "$SOURCE" == "auto" && "$TAG_NAME" != "latest" ]]; then
  if [[ "$TAG_NAME" =~ ^[0-9] ]]; then
    TAG_NAME="callisto@$TAG_NAME"
  fi
  if [[ ! "$TAG_NAME" =~ ^callisto@[0-9][0-9A-Za-z.+-]*$ ]]; then
    echo "::error::setup-callisto version must be latest, a release version such as 0.21.0, or a release tag such as callisto@0.21.0; crate names such as callisto-cli@X are no longer accepted"
    exit 1
  fi
fi

# Detect OS platform architecture for pre-built binaries
OS_NAME=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH_NAME=$(uname -m)
ASSET_NAME=""
if [[ "$OS_NAME" == "darwin" ]]; then
  case "$ARCH_NAME" in
    arm64) ASSET_NAME="callisto-aarch64-apple-darwin.tar.gz" ;;
    x86_64) ASSET_NAME="callisto-x86_64-apple-darwin.tar.gz" ;;
  esac
elif [[ "$OS_NAME" == "linux" ]]; then
  case "$ARCH_NAME" in
    x86_64) ASSET_NAME="callisto-x86_64-unknown-linux-gnu.tar.gz" ;;
    aarch64 | arm64) ASSET_NAME="callisto-aarch64-unknown-linux-gnu.tar.gz" ;;
  esac
fi

if [[ "$TAG_NAME" == "latest" ]]; then
  DOWNLOAD_URL="https://github.com/orin-dx/callisto/releases/latest/download/$ASSET_NAME"
else
  DOWNLOAD_URL="https://github.com/orin-dx/callisto/releases/download/$TAG_NAME/$ASSET_NAME"
fi
DOWNLOAD_DEST="${RUNNER_TEMP}/callisto.tar.gz"

# The archive runs with job privileges, so it is verified against the release workflow's attestation before extraction.
install_release_asset() {
  if [[ -z "$ASSET_NAME" ]]; then
    echo "::error::no prebuilt callisto asset exists for $OS_NAME/$ARCH_NAME; install it yourself: $DOCS_URL"
    exit 1
  fi
  if ! command -v gh >/dev/null 2>&1; then
    echo "::error::gh is not available, so the release attestation cannot be verified; install gh or follow: $DOCS_URL"
    exit 1
  fi
  # curl retries transient network and HTTP errors itself; a missing asset (404) is not retried.
  if ! http_status=$(curl -sSL -f --retry 3 --retry-delay 2 -w '%{http_code}' "$DOWNLOAD_URL" -o "$DOWNLOAD_DEST"); then
    rm -f "$DOWNLOAD_DEST"
    if [[ "$http_status" == "404" ]]; then
      echo "::error::no prebuilt callisto asset $ASSET_NAME exists for $TAG_NAME; install it yourself: $DOCS_URL"
    else
      echo "::error::downloading $ASSET_NAME for $TAG_NAME failed (HTTP status ${http_status:-none}) after retries; retry the job: $DOCS_URL"
    fi
    exit 1
  fi
  if ! gh attestation verify "$DOWNLOAD_DEST" \
      --repo orin-dx/callisto \
      --signer-workflow orin-dx/callisto/.github/workflows/callisto-release.yml \
      --deny-self-hosted-runners \
      --source-ref refs/heads/main; then
    rm -f "$DOWNLOAD_DEST"
    echo "::error::attestation for $ASSET_NAME did not verify; do not use this file: $DOCS_URL"
    exit 1
  fi
  # Extract only the expected binary, never the whole archive.
  tar -xzf "$DOWNLOAD_DEST" -C "$CALLISTO_BIN_DIR" callisto
  echo "Callisto installed from the verified GitHub release ($DOWNLOAD_URL)"
}

# Dependencies named callisto-cli do not establish checkout source ownership.
owns_local_source() {
  local workspace git_root metadata
  workspace=$(cd "$GITHUB_WORKSPACE" && pwd -P) || return 1
  git_root=$(git -C "$workspace" rev-parse --show-toplevel) || return 1
  [[ "$git_root" == "$workspace" ]] || return 1
  git -C "$workspace" ls-files --error-unmatch -- \
    Cargo.toml crates/callisto-cli/Cargo.toml crates/callisto-cli/src/main.rs >/dev/null || return 1
  metadata=$(cargo metadata --locked --offline --no-deps --format-version 1 \
    --manifest-path "$workspace/Cargo.toml") || return 1
  jq -e --arg workspace "$workspace" '
    .workspace_root == $workspace and
    (.workspace_members as $members | [.packages[] |
      select(.name == "callisto-cli" and .source == null and
        .manifest_path == ($workspace + "/crates/callisto-cli/Cargo.toml") and
        (.id as $id | $members | index($id) != null) and
        any(.targets[]; .name == "callisto" and (.kind | index("bin") != null) and
          .src_path == ($workspace + "/crates/callisto-cli/src/main.rs")))] | length == 1)
  ' <<< "$metadata" >/dev/null
}

if [[ "$SOURCE" == "local" ]]; then
  if ! owns_local_source; then
    echo "::error::setup-callisto source=local requires a Callisto workspace checkout; check out Callisto source or use source=auto"
    exit 1
  fi
  echo "Building local callisto binary from current repository commit..."
  cargo build --locked --manifest-path "$GITHUB_WORKSPACE/Cargo.toml" -p callisto-cli \
    --bin callisto --target-dir "$GITHUB_WORKSPACE/target"
  cp "$GITHUB_WORKSPACE/target/debug/callisto" "$CALLISTO_BIN_DIR/callisto"
else
  install_release_asset
fi

echo "$CALLISTO_BIN_DIR" >> $GITHUB_PATH
if [[ -f "$GITHUB_WORKSPACE/.github/callisto-problem-matcher.json" ]]; then
  echo "::add-matcher::$GITHUB_WORKSPACE/.github/callisto-problem-matcher.json"
fi
