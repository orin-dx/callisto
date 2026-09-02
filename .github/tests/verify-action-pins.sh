#!/usr/bin/env bash
# Local actions are commit-bound by the checkout. Every external GitHub action
# must use its full immutable commit hash; Docker actions must use an immutable
# content digest.
set -euo pipefail

status=0
while IFS= read -r line; do
  reference=${line#*uses: }
  reference=${reference%% #*}
  reference=${reference//[[:space:]]/}
  # Workspace-relative and GitHub's self-repository references are bound to
  # local repository content; only external actions need an explicit pin.
  [[ "$reference" == ./* || "$reference" == '$/'* ]] && continue
  if [[ "$reference" == docker://* ]]; then
    valid='^docker://[^@[:space:]]+@sha256:[0-9a-f]{64}$'
  else
    valid='^[^@[:space:]]+@[0-9a-f]{40}$'
  fi
  if [[ ! "$reference" =~ $valid ]]; then
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
