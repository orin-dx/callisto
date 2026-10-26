#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
if command -v yq > /dev/null; then
  yq -r '.jobs.drain.steps[] | select(.id == "pending") | .run' "$root/.github/workflows/callisto-release.yml" > "$dir/drain.sh"
else
  python3 - "$root/.github/workflows/callisto-release.yml" > "$dir/drain.sh" <<'PY'
import sys
import yaml
workflow = yaml.safe_load(open(sys.argv[1]))
print(next(step["run"] for step in workflow["jobs"]["drain"]["steps"] if step.get("id") == "pending"))
PY
fi

mkdir -p "$dir/repo/.github/scripts"
cp "$root/.github/scripts/find-pending-release.sh" "$dir/repo/.github/scripts/"
cat > "$dir/gh" <<'SH'
#!/usr/bin/env bash
case "$1 $2" in
  'pr list') cat "$PR_LIST_FILE" ;;
  'workflow run') printf '%s\n' "$*" >> "$GH_CALLS"; exit "${DISPATCH_CODE:-0}" ;;
  *) exit 90 ;;
esac
SH
chmod +x "$dir/gh"
export PATH="$dir:$PATH" GITHUB_REPOSITORY=owner/repo
export PR_LIST_FILE="$dir/prs.json" GH_CALLS="$dir/calls" GITHUB_OUTPUT="$dir/output"
export RELEASE_SOURCE_SHA=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
next=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb

cat > "$PR_LIST_FILE" <<JSON
[
  {"number":1,"mergedAt":"2026-10-24T09:00:00Z","mergeCommit":{"oid":"$RELEASE_SOURCE_SHA"},"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"owner/repo"},"baseRefName":"main","labels":[{"name":"callisto: pending"}]},
  {"number":2,"mergedAt":"2026-10-25T09:00:00Z","mergeCommit":{"oid":"$next"},"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"owner/repo"},"baseRefName":"main","labels":[{"name":"callisto: pending"}]}
]
JSON
(cd "$dir/repo" && bash "$dir/drain.sh")
grep -Fxq "workflow run callisto-release.yml --ref main -f release_source_sha=$next" "$GH_CALLS"
grep -Fxq 'has_pending=true' "$GITHUB_OUTPUT"

printf '[]\n' > "$PR_LIST_FILE"
: > "$GH_CALLS"
: > "$GITHUB_OUTPUT"
(cd "$dir/repo" && bash "$dir/drain.sh")
grep -Fxq 'has_pending=false' "$GITHUB_OUTPUT"
if [[ -s "$GH_CALLS" ]]; then
  echo 'an empty queue dispatched a release' >&2
  exit 1
fi

cat > "$PR_LIST_FILE" <<JSON
[{"number":2,"mergedAt":"2026-10-25T09:00:00Z","mergeCommit":{"oid":"$next"},"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"owner/repo"},"baseRefName":"main","labels":[{"name":"callisto: pending"}]}]
JSON
: > "$GITHUB_OUTPUT"
export DISPATCH_CODE=1
if (cd "$dir/repo" && bash "$dir/drain.sh") > "$dir/stdout" 2> "$dir/stderr"; then
  echo 'failed dispatch reported a completed drain' >&2
  exit 1
fi
if [[ -s "$GITHUB_OUTPUT" ]]; then
  echo 'a failed dispatch reported a completed drain' >&2
  exit 1
fi

echo 'release drain checks passed'
