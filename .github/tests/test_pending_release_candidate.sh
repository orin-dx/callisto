#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
script="$root/.github/scripts/find-pending-release.sh"
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
cat > "$dir/gh" <<'SH'
#!/usr/bin/env bash
[[ " $* " == *' pr list --state merged --base main --label callisto: pending '* ]] || exit 90
cat "$PR_LIST_FILE"
SH
chmod +x "$dir/gh"
export PATH="$dir:$PATH" PR_LIST_FILE="$dir/prs.json"

cat > "$PR_LIST_FILE" <<'JSON'
[
  {"number":9,"mergedAt":"2026-10-25T09:00:00Z","mergeCommit":{"oid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"owner/repo"},"baseRefName":"main","labels":[{"name":"callisto: pending"}]},
  {"number":4,"mergedAt":"2026-10-24T09:00:00Z","mergeCommit":{"oid":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"headRefName":"callisto/version-packages--1234567890abcdef1234567890abcdef12345678","headRepository":{"nameWithOwner":"owner/repo"},"baseRefName":"main","labels":[{"name":"callisto: pending"}]},
  {"number":1,"mergedAt":"2026-10-21T09:00:00Z","mergeCommit":{"oid":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"},"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"owner/repo"},"baseRefName":"main","labels":[{"name":"callisto: pending"},{"name":"callisto: complete"}]},
  {"number":3,"mergedAt":"2026-10-23T09:00:00Z","mergeCommit":{"oid":"cccccccccccccccccccccccccccccccccccccccc"},"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"fork/repo"},"baseRefName":"main"},
  {"number":2,"mergedAt":"2026-10-22T09:00:00Z","mergeCommit":{"oid":"dddddddddddddddddddddddddddddddddddddddd"},"headRefName":"other","headRepository":{"nameWithOwner":"owner/repo"},"baseRefName":"main"},
  {"number":6,"mergedAt":"2026-10-20T09:00:00Z","mergeCommit":{"oid":"ffffffffffffffffffffffffffffffffffffffff"},"headRefName":"1234567890abcdef1234567890abcdef12345678","headRepository":{"nameWithOwner":"owner/repo"},"baseRefName":"main","labels":[{"name":"callisto: pending"}]}
]
JSON
if [[ "$("$script" owner/repo main 'callisto: pending')" != bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb ]]; then
  echo 'an unrelated branch was selected as a pending release' >&2
  exit 1
fi
if [[ "$("$script" owner/repo main 'callisto: pending' bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb)" != aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ]]; then
  echo 'excluding the first merge did not select the next managed release' >&2
  exit 1
fi

printf '[]\n' > "$PR_LIST_FILE"
if [[ -n "$("$script" owner/repo main 'callisto: pending')" ]]; then
  echo 'an empty pending queue returned a source' >&2
  exit 1
fi

cat > "$PR_LIST_FILE" <<'JSON'
[{"number":9,"mergedAt":"2026-10-25T09:00:00Z","mergeCommit":null,"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"owner/repo"},"baseRefName":"main"}]
JSON
if "$script" owner/repo main 'callisto: pending' > "$dir/out" 2> "$dir/err"; then
  echo 'a pending managed release without a merge commit was silently skipped' >&2
  exit 1
fi
grep -q 'merge commit' "$dir/err"

echo 'pending release candidate checks passed'
