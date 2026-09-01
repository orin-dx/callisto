#!/usr/bin/env bash
# Local actions are commit-bound by the checkout. Every external action must
# instead use its full immutable commit hash.
set -euo pipefail

status=0
while IFS= read -r line; do
  reference=${line#*uses: }
  reference=${reference%% #*}
  reference=${reference//[[:space:]]/}
  [[ "$reference" == ./* ]] && continue
  if [[ ! "$reference" =~ ^[^@[:space:]]+@[0-9a-f]{40}$ ]]; then
    printf 'mutable or malformed external action reference: %s\n' "$line" >&2
    status=1
  fi
done < <(rg --no-heading --glob '*.yml' --glob '*.yaml' '^\s*uses:\s*' .github)
exit "$status"
