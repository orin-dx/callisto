#!/usr/bin/env bash
# The inputs of callisto-action and setup-callisto are a stable surface: each action.yml must list exactly the inputs
# (name, required, default) recorded in .github/tests/action-inputs/<action>.txt. After a deliberate change to an input,
# regenerate with `bash .github/tests/verify-action-inputs.sh --print <action>` and review the diff.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)

# One `name|required|default` line per input of the top-level `inputs:` block, in file order.
extract_inputs() {
  awk '
    function flush() { if (name != "") printf "%s|%s|%s\n", name, required, def; name = "" }
    /^inputs:[[:space:]]*$/ { in_inputs = 1; next }
    in_inputs && /^[^[:space:]#]/ { flush(); in_inputs = 0 }
    !in_inputs { next }
    /^  [A-Za-z0-9_-]+:[[:space:]]*$/ { flush(); name = $1; sub(/:$/, "", name); required = "<unset>"; def = "<unset>"; next }
    /^    required:/ { required = $0; sub(/^    required:[[:space:]]*/, "", required); gsub(/^["\047]|["\047]$/, "", required); next }
    /^    default:/ { def = $0; sub(/^    default:[[:space:]]*/, "", def); gsub(/^["\047]|["\047]$/, "", def); next }
    END { flush() }
  ' "$1"
}

if [[ "${1:-}" == --print ]]; then
  extract_inputs "$root/.github/actions/$2/action.yml"
  exit 0
fi

status=0
for action in callisto-action setup-callisto; do
  expected="$root/.github/tests/action-inputs/$action.txt"
  actual=$(extract_inputs "$root/.github/actions/$action/action.yml")
  if [[ -z "$actual" ]]; then
    printf '%s: no inputs found in action.yml\n' "$action" >&2
    status=1
    continue
  fi
  recorded=$(cat "$expected")
  if [[ "$actual" != "$recorded" ]]; then
    printf '%s: recorded inputs:\n%s\naction.yml inputs:\n%s\n' "$action" "$recorded" "$actual" >&2
    printf '%s: inputs differ from %s. Inputs are stable from 1.0: removing or renaming one is a breaking change; update the list only for a deliberate addition.\n' "$action" "${expected#"$root"/}" >&2
    status=1
  fi
done
exit "$status"
