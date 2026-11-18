#!/usr/bin/env bash
# Fails fast when callisto-action's `mode` input is not one of its accepted values.
set -euo pipefail

case "$INPUT_MODE" in
  version-pr|release|execute) ;;
  *)
    echo "::error::callisto-action mode must be 'version-pr', 'release' or 'execute', got '$INPUT_MODE'"
    exit 1
    ;;
esac
