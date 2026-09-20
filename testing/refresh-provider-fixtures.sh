#!/usr/bin/env bash
# Captures real provider responses (read-only GETs, no credentials for the
# public endpoints) into testing/fixtures/providers/. Every trimming or
# derivation rule below is restated in that directory's PROVENANCE.md.
#
#   testing/refresh-provider-fixtures.sh           rewrite the committed fixtures
#   testing/refresh-provider-fixtures.sh --check   re-fetch to a temp dir and
#                                                  assert the SHAPE still matches
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
committed=$here/fixtures/providers
mode=refresh
out=$committed
if [[ ${1:-} == --check ]]; then
  mode=check
  out=$(mktemp -d)
fi

for tool in curl gh git jq; do
  command -v "$tool" >/dev/null || { echo "missing required tool: $tool" >&2; exit 2; }
done

mkdir -p "$out"/{crates-io,pypi,github,git}

# Splits a raw HTTP response into $1.head (through the blank line) and $1.body.
split_http() {
  local raw=$1 blank
  blank=$(grep -n -m1 -E $'^\r?$' "$raw" | cut -d: -f1)
  head -n "$blank" "$raw" >"$raw.head"
  tail -n +"$((blank + 1))" "$raw" >"$raw.body"
}

# Removes stale or account-specific headers from a .head file.
drop_headers() {
  local file=$1 pattern=$2
  grep -v -i -E "$pattern" "$file" >"$file.tmp" || true
  mv "$file.tmp" "$file"
}

work=$(mktemp -d)

# --- crates.io sparse index -------------------------------------------------
curl -sS --include https://index.crates.io/ca/ll/callisto-model >"$work/idx"
split_http "$work/idx"
head -n 3 "$work/idx.body" >"$work/idx.trim"
drop_headers "$work/idx.head" '^content-length:'
cat "$work/idx.head" "$work/idx.trim" >"$out/crates-io/sparse-index-200.http"

curl -sS --include https://index.crates.io/ca/ll/callisto-model-no-such-crate-zz >"$out/crates-io/sparse-index-404.http"

curl -sS https://index.crates.io/ch/ac/chacha20 | grep -F '"vers":"0.10.1"' >"$out/crates-io/sparse-index-yanked-line.json"
[[ $(jq -r .yanked <"$out/crates-io/sparse-index-yanked-line.json") == true ]] \
  || { echo "chacha20 0.10.1 is no longer yanked; pick another yanked version" >&2; exit 1; }

# --- PyPI -------------------------------------------------------------------
curl -sS --include https://pypi.org/pypi/requests/2.31.0/json >"$work/pypi"
split_http "$work/pypi"
drop_headers "$work/pypi.head" '^content-length:'
jq -c '{info: (.info | with_entries(select(.key | IN("name","version","summary","yanked","yanked_reason","requires_python")))), last_serial, urls}' \
  <"$work/pypi.body" >"$work/pypi.trim"
cat "$work/pypi.head" "$work/pypi.trim" >"$out/pypi/version-200.http"

curl -sS --include https://pypi.org/pypi/requests-no-such-project-zz/2.31.0/json >"$out/pypi/version-404.http"

# --- GitHub REST via gh api --include ---------------------------------------
ACCOUNT_HEADERS='^x-oauth-|^x-accepted-oauth-'
trim_release='.assets |= .[:2] | del(.body)'

gh api --include repos/cli/cli/releases/latest >"$work/rel"
split_http "$work/rel"
drop_headers "$work/rel.head" "^content-length:|$ACCOUNT_HEADERS"
jq -c "$trim_release" <"$work/rel.body" >"$work/rel.trim"
cat "$work/rel.head" "$work/rel.trim" >"$out/github/release-published.http"
jq -c '.draft = true' <"$work/rel.trim" >"$work/rel.draft"
cat "$work/rel.head" "$work/rel.draft" >"$out/github/release-draft.http"
jq -c '.prerelease = true' <"$work/rel.trim" >"$work/rel.pre"
cat "$work/rel.head" "$work/rel.pre" >"$out/github/release-prerelease.http"

gh api --include 'repos/cli/cli/releases?per_page=2&page=1' >"$work/list"
split_http "$work/list"
drop_headers "$work/list.head" "^content-length:|$ACCOUNT_HEADERS"
jq -c "map($trim_release)" <"$work/list.body" >"$work/list.trim"
cat "$work/list.head" "$work/list.trim" >"$out/github/release-list-page.http"

gh api --include repos/cli/cli/releases/tags/callisto-no-such-tag >"$work/nf" 2>/dev/null || true
split_http "$work/nf"
drop_headers "$work/nf.head" "^content-length:|$ACCOUNT_HEADERS"
cat "$work/nf.head" "$work/nf.body" >"$out/github/release-404.http"

# --- git ls-remote ----------------------------------------------------------
remote=https://github.com/orin-dx/callisto.git
ls_remote() { git ls-remote "$remote" "refs/tags/$1" "refs/tags/$1^{}"; }
ls_remote 'callisto-changelog@0.3.1' >"$out/git/ls-remote-annotated.txt"
ls_remote 'callisto-vcs@0.2.0' >"$out/git/ls-remote-lightweight.txt"
ls_remote 'callisto-no-such-tag' >"$out/git/ls-remote-absent.txt"
[[ $(wc -l <"$out/git/ls-remote-annotated.txt") -eq 2 ]] || { echo "annotated tag lost its peeled line" >&2; exit 1; }
[[ $(wc -l <"$out/git/ls-remote-lightweight.txt") -eq 1 ]] || { echo "lightweight tag gained a peeled line" >&2; exit 1; }

# --- shape comparison -------------------------------------------------------
# A file's shape is its HTTP status code plus the set of JSON key paths (array
# indices collapsed), or for ls-remote the per-line ref kind. Content is never
# compared: versions, digests and dates are expected to drift.
shape_of() {
  local file=$1
  case $file in
    *.http)
      head -n 1 "$file" | awk '{print "status", $2}'
      local blank
      blank=$(grep -n -m1 -E $'^\r?$' "$file" | cut -d: -f1)
      tail -n +"$((blank + 1))" "$file" | jq -r '[paths | map(if type == "number" then "[]" else . end | tostring) | join(".")] | unique | .[]' 2>/dev/null || echo "non-json-body"
      ;;
    *.json)
      jq -r '[paths | map(if type == "number" then "[]" else . end | tostring) | join(".")] | unique | .[]' <"$file"
      ;;
    *.txt)
      awk '{kind = ($2 ~ /\^\{\}$/) ? "peeled" : "ref"; print kind, length($1)}' "$file"
      ;;
  esac
}

if [[ $mode == check ]]; then
  failed=0
  while IFS= read -r file; do
    rel=${file#"$committed"/}
    [[ $rel == PROVENANCE.md || $rel == */PROVENANCE.md ]] && continue
    if ! diff <(shape_of "$file") <(shape_of "$out/$rel") >"$work/diff"; then
      echo "SHAPE DRIFT: $rel"
      cat "$work/diff"
      failed=1
    else
      echo "shape ok:    $rel"
    fi
  done < <(find "$committed" -type f ! -name PROVENANCE.md ! -name .gitattributes | sort)
  exit "$failed"
fi

echo "captured $(date +%F) with: $(gh --version | head -1); $(git --version); $(curl --version | head -1); $(jq --version)"
find "$out" -type f ! -name PROVENANCE.md ! -name .gitattributes | sort | while IFS= read -r file; do
  printf '%7s bytes  %s\n' "$(wc -c <"$file" | tr -d ' ')" "${file#"$here"/}"
done
