#!/usr/bin/env bash
# Fails fast when callisto-action's `mode` input is not one of its two accepted values.
set -euo pipefail

case "$INPUT_MODE" in
  version-pr|release) ;;
  *)
    echo "::error::callisto-action mode must be 'version-pr' or 'release', got '$INPUT_MODE'"
    exit 1
    ;;
esac
