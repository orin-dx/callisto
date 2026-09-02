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
codeowners=.github/CODEOWNERS
if [[ ! -f "$codeowners" ]] \
  || ! rg -q '^/\.github/workflows/\s+@[^[:space:]]+$' "$codeowners" \
  || ! rg -q '^/\.github/actions/\s+@[^[:space:]]+$' "$codeowners"; then
  printf 'CODEOWNERS must cover .github/workflows/ and .github/actions/ with a real owner\n' >&2
  status=1
fi
exit "$status"
