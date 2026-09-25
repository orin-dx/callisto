#!/usr/bin/env bash
# Build one declared Callisto product artifact. This script deliberately owns
# only platform compilation and archive layout; Callisto owns all release
# planning, artifact identity, provenance verification, and publication.
set -euo pipefail

: "${CALLISTO_RELEASE_ARTIFACT_KIND:?must name cli or cross}"
: "${CALLISTO_RELEASE_ARTIFACT_TARGET:?must name a supported Rust target}"
: "${CALLISTO_RELEASE_ARTIFACT_ASSET:?must name the expected release asset}"
: "${CALLISTO_RELEASE_SOURCE_ROOT:?must name the checked-out source root}"
: "${CALLISTO_RELEASE_ARTIFACT_DIR:?must name an output directory}"

kind="$CALLISTO_RELEASE_ARTIFACT_KIND"
target="$CALLISTO_RELEASE_ARTIFACT_TARGET"
asset="$CALLISTO_RELEASE_ARTIFACT_ASSET"
source_root="$CALLISTO_RELEASE_SOURCE_ROOT"
artifact_dir="$CALLISTO_RELEASE_ARTIFACT_DIR"

case "$kind:$target:$asset" in
  cli:aarch64-apple-darwin:callisto-aarch64-apple-darwin.tar.gz | \
  cli:x86_64-unknown-linux-gnu:callisto-x86_64-unknown-linux-gnu.tar.gz | \
  cross:x86_64-unknown-linux-musl:callisto-x86_64-unknown-linux-musl.tar.gz)
    ;;
  *)
    printf 'unsupported Callisto release artifact: kind=%s target=%s asset=%s\n' \
      "$kind" "$target" "$asset" >&2
    exit 1
    ;;
esac

mkdir -p "$artifact_dir"

case "$kind" in
  cli)
    rustup target add "$target"
    (cd "$source_root" && cargo build --locked --release -p callisto-cli --target "$target")
    tar -C "$source_root/target/$target/release" -czf "$artifact_dir/$asset" callisto
    ;;
  cross)
    rustup target add "$target"
    cargo install cross --locked --version 0.2.5
    (cd "$source_root" && cross build --locked --release -p callisto-cli --target "$target")
    tar -C "$source_root/target/$target/release" -czf "$artifact_dir/$asset" callisto
    ;;
esac
