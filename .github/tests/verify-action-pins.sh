#!/usr/bin/env bash
# Local actions are commit-bound by the checkout. Every external GitHub action
# must use its full immutable commit hash; Docker actions must use an immutable
# content digest. The generated-workflow templates are checked too, and one
# action resolves to one commit across the repository and its templates.
set -euo pipefail

status=0
pairs=''
while IFS= read -r line; do
  reference=${line#*uses: }
  reference=${reference%% #*}
  reference=${reference//[[:space:]]/}
  # Workspace-relative and GitHub's self-repository references are bound to
  # local repository content; only external actions need an explicit pin.
  [[ "$reference" == ./* || "$reference" == '$/'* ]] && continue
  # A template placeholder is replaced with a commit when the workflow is rendered.
  [[ "$reference" =~ @__[A-Z_]+__$ ]] && continue
  if [[ "$reference" == docker://* ]]; then
    valid='^docker://[^@[:space:]]+@sha256:[0-9a-f]{64}$'
  else
    valid='^[^@[:space:]]+@[0-9a-f]{40}$'
  fi
  if [[ ! "$reference" =~ $valid ]]; then
    printf 'mutable or malformed external action reference: %s\n' "$line" >&2
    status=1
    continue
  fi
  pairs+="${reference%@*} ${reference##*@}"$'\n'
done < <(rg --no-heading --no-filename --glob '*.yml' --glob '*.yaml' --glob '*.tmpl' '^\s*(- )?uses:\s*' .github crates/callisto-graph/src/templates)
drift=$(printf '%s' "$pairs" | sort -u | awk '{ seen[$1] = seen[$1] " " $2; count[$1]++ } END { for (name in count) if (count[name] > 1) print name ":" seen[name] }')
if [[ -n "$drift" ]]; then
  printf 'an action is pinned to more than one commit:\n%s\n' "$drift" >&2
  status=1
fi
codeowners=.github/CODEOWNERS
if [[ ! -f "$codeowners" ]] \
  || ! rg -q '^/\.github/workflows/\s+@[^[:space:]]+$' "$codeowners" \
  || ! rg -q '^/\.github/actions/\s+@[^[:space:]]+$' "$codeowners"; then
  printf 'CODEOWNERS must cover .github/workflows/ and .github/actions/ with a real owner\n' >&2
  status=1
fi
exit "$status"
