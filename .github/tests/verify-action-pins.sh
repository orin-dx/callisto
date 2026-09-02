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
done < <(
  # This verifier runs in the release job, whose minimal toolchain does not
  # promise ripgrep. Traverse only tracked workflow/action YAML with Git and
  # use the ubiquitous grep implementation provided by the runner image.
  git ls-files -- .github | while IFS= read -r file; do
    if [[ "$file" == *.yml || "$file" == *.yaml ]]; then
      grep -E '^[[:space:]]*uses:[[:space:]]*' "$file"
    fi
  done
)
codeowners=.github/CODEOWNERS
if [[ ! -f "$codeowners" ]] \
  || ! grep -Eq '^/\.github/workflows/[[:space:]]+@[^[:space:]]+$' "$codeowners" \
  || ! grep -Eq '^/\.github/actions/[[:space:]]+@[^[:space:]]+$' "$codeowners"; then
  printf 'CODEOWNERS must cover .github/workflows/ and .github/actions/ with a real owner\n' >&2
  status=1
fi
exit "$status"
