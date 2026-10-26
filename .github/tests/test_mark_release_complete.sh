#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
script="$root/.github/scripts/mark-release-complete.sh"
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
cat > "$dir/gh" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$GH_CALLS"
case "$1 $2" in
  'pr view') cat "$PR_FILE" ;;
  'label list') echo true ;;
  'pr edit') ;;
  *) exit 90 ;;
esac
SH
chmod +x "$dir/gh"
export PATH="$dir:$PATH" GH_CALLS="$dir/calls" PR_FILE="$dir/pr"
sha=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa

cat > "$PR_FILE" <<JSON
{"mergedAt":"2026-10-25T09:00:00Z","mergeCommit":{"oid":"$sha"},"labels":[{"name":"callisto: pending"}]}
JSON
bash "$script" 9 "$sha"
grep -Fq 'pr edit 9 --remove-label callisto: pending --add-label callisto: complete' "$GH_CALLS"

: > "$GH_CALLS"
if bash "$script" 9 bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb > "$dir/out" 2> "$dir/err"; then
  echo 'a different merge commit changed the PR labels' >&2
  exit 1
fi
if grep -Fq 'pr edit' "$GH_CALLS"; then
  echo 'source mismatch edited release PR labels' >&2
  exit 1
fi

cat > "$PR_FILE" <<JSON
{"mergedAt":"2026-10-25T09:00:00Z","mergeCommit":{"oid":"$sha"},"labels":[{"name":"callisto: complete"}]}
JSON
: > "$GH_CALLS"
bash "$script" 9 "$sha"
if grep -Fq 'pr edit' "$GH_CALLS"; then
  echo 'completed release PR labels were edited again' >&2
  exit 1
fi

echo 'release PR completion checks passed'
