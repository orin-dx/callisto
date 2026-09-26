#!/usr/bin/env bash
# Regression test for the github.action_ref -> callisto@<version> resolution
# in scripts/install-callisto.sh: a workflow pinned to
# `callisto-action@<sha> # callisto@X.Y.Z` (or setup-callisto the same way)
# must install X.Y.Z, not whatever GitHub Releases calls "latest". Extracts
# the snippet by real text anchors so this test always exercises the
# script's current logic.
set -u
ACTION_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_SCRIPT="$ACTION_DIR/scripts/install-callisto.sh"

extract_snippet() {
  sed -n '/^TAG_NAME="\$INPUT_CALLISTO_VERSION"$/,/^# Validate verification mode$/p' "$INSTALL_SCRIPT" \
    | sed '$d'
}

# Runs the extracted snippet with INPUT_CALLISTO_VERSION/GITHUB_ACTION_REF set
# and `git ls-remote` stubbed to print tag_lines; prints the resulting
# TAG_NAME and records every git invocation.
run_case() {
  local input_version="$1" action_ref="$2" tag_lines="$3"
  local tmp_script calls_file
  tmp_script="$(mktemp)"
  calls_file="$(mktemp)"
  {
    printf 'git() { echo "git $*" >> %q; if [[ "$1 $2" == "ls-remote --tags" ]]; then printf %%s %q; fi; }\n' \
      "$calls_file" "$tag_lines"
    printf 'INPUT_CALLISTO_VERSION=%q\n' "$input_version"
    [[ -n "$action_ref" ]] && printf 'GITHUB_ACTION_REF=%q\n' "$action_ref"
    printf 'INPUT_CALLISTO_SOURCE=auto\n'
    extract_snippet
    printf 'echo "TAG_NAME=$TAG_NAME"\n'
  } > "$tmp_script"
  bash "$tmp_script"
  echo "---CALLS---"
  cat "$calls_file"
  rm -f "$tmp_script" "$calls_file"
}

fail=0
TAGS=$'deadbeef00000000000000000000000000000001\trefs/tags/callisto@0.7.0\nabc1230000000000000000000000000000000002\trefs/tags/callisto@0.8.0\nabc1230000000000000000000000000000000002\trefs/tags/callisto@0.8.0^{}\n'

# 1. No action ref set (local/unpinned invocation): stays "latest", no git call.
out=$(run_case "latest" "" "$TAGS")
if [[ "$out" != *"TAG_NAME=latest"* ]] || [[ "$out" == *"git ls-remote"* ]]; then
  echo "FAIL test_no_action_ref_stays_latest_without_git_call: $out"; fail=1
else
  echo "PASS test_no_action_ref_stays_latest_without_git_call"
fi

# 2. Action ref matches a published callisto@ tag: resolved, dereferenced ^{} ignored.
out=$(run_case "latest" "abc1230000000000000000000000000000000002" "$TAGS")
if [[ "$out" != *"TAG_NAME=callisto@0.8.0"* ]]; then
  echo "FAIL test_matching_action_ref_resolves_to_tag: $out"; fail=1
else
  echo "PASS test_matching_action_ref_resolves_to_tag"
fi

# 3. Action ref matches no tag: falls back to true latest.
out=$(run_case "latest" "0000000000000000000000000000000000000f" "$TAGS")
if [[ "$out" != *"TAG_NAME=latest"* ]]; then
  echo "FAIL test_unmatched_action_ref_falls_back_to_latest: $out"; fail=1
else
  echo "PASS test_unmatched_action_ref_falls_back_to_latest"
fi

# 4. Explicit version override is never touched, even with a resolvable action ref.
out=$(run_case "callisto@0.5.0" "abc1230000000000000000000000000000000002" "$TAGS")
if [[ "$out" != *"TAG_NAME=callisto@0.5.0"* ]] || [[ "$out" == *"git ls-remote"* ]]; then
  echo "FAIL test_explicit_version_override_skips_resolution: $out"; fail=1
else
  echo "PASS test_explicit_version_override_skips_resolution"
fi

exit "$fail"
