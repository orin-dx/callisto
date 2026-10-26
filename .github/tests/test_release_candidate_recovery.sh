#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
workflow="${WORKFLOW_FILE:-$root/.github/workflows/callisto-release.yml}"
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
if command -v yq > /dev/null; then
  yq -r '.jobs."release-candidate".steps[] | select(.id == "candidate") | .run' "$workflow" > "$dir/candidate.sh"
else
  python3 - "$workflow" > "$dir/candidate.sh" <<'PY'
import sys
import yaml
workflow = yaml.safe_load(open(sys.argv[1]))
print(next(step["run"] for step in workflow["jobs"]["release-candidate"]["steps"] if step.get("id") == "candidate"))
PY
fi

cat > "$dir/gh" <<'SH'
#!/usr/bin/env bash
case "$1 $2" in
  'pr list') cat "$PR_LIST_FILE" ;;
  'api --paginate') cat "$PULLS_FILE" ;;
  api\ /repos/owner/repo/commits/*)
    case "$4" in
      .sha) echo "${2##*/}" ;;
      .commit.verification.verified) echo "${VERIFIED:-true}" ;;
      *) exit 90 ;;
    esac ;;
  *) exit 90 ;;
esac
SH
chmod +x "$dir/gh"
export PATH="$dir:$PATH" GITHUB_REPOSITORY=owner/repo
export PR_LIST_FILE="$dir/prs.json" PULLS_FILE="$dir/pulls.json"
mkdir -p "$dir/repo/.github/scripts"
cp "$root/.github/scripts/find-pending-release.sh" "$dir/repo/.github/scripts/"
git -C "$dir/repo" init -q
git -C "$dir/repo" -c user.name=Test -c user.email=test@example.com commit --allow-empty -qm source
export TEST_SOURCE_SHA
TEST_SOURCE_SHA="$(git -C "$dir/repo" rev-parse HEAD)"
git -C "$dir/repo" -c user.name=Test -c user.email=test@example.com commit --allow-empty -qm advanced-main
export TRIGGERING_SHA GITHUB_OUTPUT="$dir/output"
TRIGGERING_SHA="$(git -C "$dir/repo" rev-parse HEAD)"
printf '[]\n' > "$PULLS_FILE"

cat > "$PR_LIST_FILE" <<JSON
[{"number":9,"mergedAt":"2026-10-25T09:00:00Z","mergeCommit":{"oid":"$TEST_SOURCE_SHA"},"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"owner/repo"},"baseRefName":"main","labels":[{"name":"callisto: pending"}]}]
JSON
export RELEASE_SOURCE_SHA_INPUT=''
if (cd "$dir/repo" && bash "$dir/candidate.sh") > "$dir/stdout" 2> "$dir/stderr"; then
  echo 'a pending PR without a verified managed merge was silently skipped' >&2
  exit 1
fi
if [[ -f "$GITHUB_OUTPUT" ]] && grep -Fq 'is_release_pr=false' "$GITHUB_OUTPUT"; then
  echo 'an invalid pending PR allowed the next release PR to start' >&2
  exit 1
fi

printf '[]\n' > "$PR_LIST_FILE"
: > "$GITHUB_OUTPUT"
(cd "$dir/repo" && bash "$dir/candidate.sh")
grep -Fxq 'is_release_pr=false' "$GITHUB_OUTPUT"

: > "$GITHUB_OUTPUT"
export RELEASE_SOURCE_SHA_INPUT="$TEST_SOURCE_SHA"
if (cd "$dir/repo" && bash "$dir/candidate.sh") > "$dir/stdout" 2> "$dir/stderr"; then
  echo 'an explicit unmanaged source was accepted' >&2
  exit 1
fi

git -C "$dir/repo" checkout --orphan unrelated -q
git -C "$dir/repo" -c user.name=Test -c user.email=test@example.com commit --allow-empty -qm unrelated
unrelated_sha="$(git -C "$dir/repo" rev-parse HEAD)"
git -C "$dir/repo" checkout --detach "$TRIGGERING_SHA" -q
export RELEASE_SOURCE_SHA_INPUT="$unrelated_sha"
if (cd "$dir/repo" && bash "$dir/candidate.sh") > "$dir/stdout" 2> "$dir/stderr"; then
  echo 'a source outside main history was accepted' >&2
  exit 1
fi
if ! grep -Fq 'not on main' "$dir/stdout"; then
  cat "$dir/stdout" "$dir/stderr" >&2
  exit 1
fi
export RELEASE_SOURCE_SHA_INPUT="$TEST_SOURCE_SHA"

cat > "$PULLS_FILE" <<JSON
[{"number":9,"merged_at":"2026-10-25T09:00:00Z","merge_commit_sha":"$TRIGGERING_SHA","base":{"ref":"main"},"head":{"repo":{"full_name":"owner/repo"},"ref":"callisto/version-packages"}}]
JSON
: > "$GITHUB_OUTPUT"
if (cd "$dir/repo" && bash "$dir/candidate.sh") > "$dir/stdout" 2> "$dir/stderr"; then
  echo 'a commit associated with a managed PR but not its merge commit was accepted' >&2
  exit 1
fi

cat > "$PULLS_FILE" <<JSON
[{"number":9,"merged_at":"2026-10-25T09:00:00Z","merge_commit_sha":"$TEST_SOURCE_SHA","base":{"ref":"main"},"head":{"repo":{"full_name":"owner/repo"},"ref":"callisto/version-packages"}}]
JSON
: > "$GITHUB_OUTPUT"
(cd "$dir/repo" && bash "$dir/candidate.sh")
grep -Fxq 'is_release_pr=true' "$GITHUB_OUTPUT"
grep -Fxq 'release_pr_number=9' "$GITHUB_OUTPUT"
grep -Fxq "release_source_sha=$TEST_SOURCE_SHA" "$GITHUB_OUTPUT"
grep -Fxq "orchestration_sha=$TRIGGERING_SHA" "$GITHUB_OUTPUT"

cat > "$PR_LIST_FILE" <<JSON
[{"number":9,"mergedAt":"2026-10-25T09:00:00Z","mergeCommit":{"oid":"$TEST_SOURCE_SHA"},"headRefName":"callisto/version-packages","headRepository":{"nameWithOwner":"owner/repo"},"baseRefName":"main","labels":[{"name":"callisto: pending"}]}]
JSON
: > "$GITHUB_OUTPUT"
export RELEASE_SOURCE_SHA_INPUT=''
(cd "$dir/repo" && bash "$dir/candidate.sh")
grep -Fxq "release_source_sha=$TEST_SOURCE_SHA" "$GITHUB_OUTPUT"
grep -Fxq "orchestration_sha=$TRIGGERING_SHA" "$GITHUB_OUTPUT"

: > "$GITHUB_OUTPUT"
export VERIFIED=false
if (cd "$dir/repo" && bash "$dir/candidate.sh") > "$dir/stdout" 2> "$dir/stderr"; then
  echo 'an unverified managed merge was accepted' >&2
  exit 1
fi

echo 'release candidate recovery checks passed'
