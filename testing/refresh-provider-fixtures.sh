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

for tool in cargo npm gh git jq; do
  command -v "$tool" >/dev/null || { echo "missing required tool: $tool" >&2; exit 2; }
done

mkdir -p "$out"/{cargo-info,npm-view,github,git}

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

# Captures one command's exit code, stdout and stderr into a `.cmd` envelope,
# which is exactly what the production classifiers are fed in unit tests.
capture_cmd() {
  local target=$1
  shift
  local code=0
  "$@" >"$work/cmd.out" 2>"$work/cmd.err" || code=$?
  { printf 'exit: %s\n--- stdout\n' "$code"
    cat "$work/cmd.out"
    printf -- '--- stderr\n'
    cat "$work/cmd.err"
  } >"$target"
}

# --- cargo info (the cargo observation) -------------------------------------
# Run from a scratch package so no local workspace member can be resolved; the
# release path always passes --registry for the same reason.
probe=$work/cargo-probe
mkdir -p "$probe/src"
printf '[package]\nname = "callisto-fixture-probe"\nversion = "0.0.0"\nedition = "2021"\n' >"$probe/Cargo.toml"
: >"$probe/src/lib.rs"
cargo_info() { (cd "$probe" && cargo info "$@"); }

capture_cmd "$out/cargo-info/found.cmd" cargo_info 'serde@1.0.0' --registry crates-io
capture_cmd "$out/cargo-info/absent-version.cmd" cargo_info 'serde@0.0.0-definitely-not' --registry crates-io
capture_cmd "$out/cargo-info/unknown-crate.cmd" cargo_info 'callisto-model-no-such-crate-zz@1.0.0' --registry crates-io
capture_cmd "$out/cargo-info/yanked.cmd" cargo_info 'chacha20@0.10.1' --registry crates-io
grep -q 'could not find `chacha20@0.10.1`' "$out/cargo-info/yanked.cmd" \
  || { echo "chacha20 0.10.1 is no longer yanked or cargo changed its wording" >&2; exit 1; }

# The transport failure shape, captured against a registry index that refuses
# every connection rather than by breaking the machine's real network.
mkdir -p "$probe/.cargo"
printf '[registries.unreachable]\nindex = "sparse+https://127.0.0.1:9/index/"\n' >"$probe/.cargo/config.toml"
capture_cmd "$out/cargo-info/network-failure.cmd" cargo_info 'serde@1.0.0' --registry unreachable
rm -rf "$probe/.cargo"

# --- npm view (the npm observation) -----------------------------------------
capture_cmd "$out/npm-view/found.cmd" npm view 'left-pad@1.3.0' version --json
capture_cmd "$out/npm-view/missing.cmd" npm view 'callisto-no-such-package-zz@1.0.0' version --json
# npm's trailing debug-log line names the capturing machine's home directory.
grep -v '^npm error A complete log of this run' "$out/npm-view/missing.cmd" >"$work/npm-missing"
mv "$work/npm-missing" "$out/npm-view/missing.cmd"

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
# indices collapsed), for ls-remote the per-line ref kind, and for a captured
# command its exit code plus the first line of each stream with every digit run
# masked. Content is never compared: versions, digests and dates are expected
# to drift.
shape_of() {
  local file=$1
  case $file in
    *.cmd)
      grep -m1 '^exit: ' "$file"
      awk '/^--- stdout$/ {s=1; next} /^--- stderr$/ {s=2; next}
           s == 1 && !o {print "stdout", $0; o = 1}
           s == 2 && !e {print "stderr", $0; e = 1}' "$file" | sed -E 's/[0-9]+/N/g'
      ;;
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

echo "captured $(date +%F) with: $(cargo --version); npm $(npm --version); $(gh --version | head -1); $(git --version); $(jq --version)"
find "$out" -type f ! -name PROVENANCE.md ! -name .gitattributes | sort | while IFS= read -r file; do
  printf '%7s bytes  %s\n' "$(wc -c <"$file" | tr -d ' ')" "${file#"$here"/}"
done
