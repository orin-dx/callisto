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

# 3b. A named ref (the floating v1 tag) resolves through the commit it points at, lightweight or annotated.
FLOATING=$'abc1230000000000000000000000000000000002\trefs/tags/callisto@0.8.0\nabc1230000000000000000000000000000000002\trefs/tags/v1\nfeed000000000000000000000000000000000003\trefs/tags/v2\nabc1230000000000000000000000000000000002\trefs/tags/v2^{}\n'
out=$(run_case "latest" "v1" "$FLOATING")
if [[ "$out" != *"TAG_NAME=callisto@0.8.0"* ]]; then
  echo "FAIL test_floating_tag_ref_resolves_to_its_release: $out"; fail=1
else
  echo "PASS test_floating_tag_ref_resolves_to_its_release"
fi
out=$(run_case "latest" "v2" "$FLOATING")
if [[ "$out" != *"TAG_NAME=callisto@0.8.0"* ]]; then
  echo "FAIL test_annotated_floating_tag_ref_uses_its_peeled_commit: $out"; fail=1
else
  echo "PASS test_annotated_floating_tag_ref_uses_its_peeled_commit"
fi
out=$(run_case "latest" "v9" "$FLOATING")
if [[ "$out" != *"TAG_NAME=latest"* ]]; then
  echo "FAIL test_unknown_named_ref_falls_back_to_latest: $out"; fail=1
else
  echo "PASS test_unknown_named_ref_falls_back_to_latest"
fi

# 4. Explicit version override is never touched, even with a resolvable action ref.
out=$(run_case "callisto@0.5.0" "abc1230000000000000000000000000000000002" "$TAGS")
if [[ "$out" != *"TAG_NAME=callisto@0.5.0"* ]] || [[ "$out" == *"git ls-remote"* ]]; then
  echo "FAIL test_explicit_version_override_skips_resolution: $out"; fail=1
else
  echo "PASS test_explicit_version_override_skips_resolution"
fi

run_installer() (
  local source="$1" version="$2" checkout="$3" action_ref="${4:-}"
  local fixture code
  fixture=$(mktemp -d)
  fixture=$(cd "$fixture" && pwd -P)
  export GITHUB_WORKSPACE="$fixture/checkout with spaces" RUNNER_TEMP="$fixture/runner"
  export GITHUB_PATH="$fixture/github-path" GITHUB_ACTION_REF="$action_ref"
  export INPUT_CALLISTO_SOURCE="$source" INPUT_CALLISTO_VERSION="$version" INPUT_CALLISTO_VERIFICATION=skip
  mkdir -p "$GITHUB_WORKSPACE/crates/callisto-cli/src" "$RUNNER_TEMP"
  printf '# consumer depends on callisto-cli\n' > "$GITHUB_WORKSPACE/Cargo.toml"
  : > "$GITHUB_WORKSPACE/crates/callisto-cli/Cargo.toml"
  : > "$GITHUB_WORKSPACE/crates/callisto-cli/src/main.rs"
  : > "$fixture/calls"
  git() {
    echo "git $*" >> "$fixture/calls"
    case "$*" in
      ls-remote*) printf '%s' "$TAGS" ;;
      *rev-parse*) [[ "$checkout" != untracked ]] && printf '%s\n' "$GITHUB_WORKSPACE" ;;
      *ls-files*) [[ "$checkout" != untracked ]] ;;
      *) return 99 ;;
    esac
  }
  cargo() {
    echo "cargo $*" >> "$fixture/calls"
    case "$1" in
      metadata)
        [[ "$checkout" != metadata-failure ]] || return 1
        command jq -n --arg root "$GITHUB_WORKSPACE" --arg mode "$checkout" '
          {workspace_root: (if $mode == "foreign-root" then $root + "/other" else $root end),
           workspace_members: (if $mode == "nonmember" then [] else ["cli-id"] end),
           packages: [{id: "cli-id", name: "callisto-cli", source: null,
             manifest_path: (if $mode == "dependency" then "/registry/Cargo.toml"
               else $root + "/crates/callisto-cli/Cargo.toml" end),
             targets: [{name: "callisto", kind: ["bin"],
               src_path: (if $mode == "foreign-target" then "/other/main.rs"
                 else $root + "/crates/callisto-cli/src/main.rs" end)}]}]}'
        ;;
      build) ;;
      install) [[ "$checkout" != install-failure ]] ;;
      *) return 99 ;;
    esac
  }
  jq() { echo "jq $*" >> "$fixture/calls"; command jq "$@"; }
  uname() { [[ "$1" == -s ]] && echo Linux || echo x86_64; }
  curl() { echo "curl $*" >> "$fixture/calls"; return 1; }
  gh() { echo "unexpected gh $*" >> "$fixture/calls"; return 99; }
  tar() { echo "unexpected tar $*" >> "$fixture/calls"; return 99; }
  unzip() { echo "unexpected unzip $*" >> "$fixture/calls"; return 99; }
  cp() { echo "cp $*" >> "$fixture/calls"; }
  (cd "$fixture" && source "$INSTALL_SCRIPT")
  code=$?
  cat "$fixture/calls"
  rm -rf "$fixture"
  return "$code"
)

for checkout in dependency owned; do
  out=$(run_installer auto callisto@0.5.0 "$checkout"); code=$?
  if [[ $code -ne 0 || "$out" != *"cargo install callisto-cli --version 0.5.0 "* \
    || "$out" == *"cargo build"* || "$out" == *"cargo metadata"* || "$out" == *"git ls-remote"* ]]; then
    echo "FAIL auto_explicit_pin_$checkout: code=$code out=$out"; fail=1
  else
    echo "PASS auto_explicit_pin_$checkout"
  fi
done

out=$(run_installer auto latest owned abc1230000000000000000000000000000000002); code=$?
if [[ $code -ne 0 || "$out" != *"cargo install callisto-cli --version 0.8.0 "* || "$out" == *"cargo build"* ]]; then
  echo "FAIL auto_action_ref_pin_owned: code=$code out=$out"; fail=1
else
  echo "PASS auto_action_ref_pin_owned"
fi

out=$(run_installer local latest owned abc1230000000000000000000000000000000002); code=$?
if [[ $code -ne 0 || "$out" != *"cargo metadata --locked --offline --no-deps --format-version 1 --manifest-path "* \
  || "$out" != *"cargo build --locked --manifest-path "* || "$out" != *"--target-dir "*"/checkout with spaces/target"* \
  || "$out" != *"cp "*"/checkout with spaces/target/debug/callisto "* \
  || "$out" == *"git ls-remote"* || "$out" == *"cargo install"* || "$out" == *"curl "* ]]; then
  echo "FAIL local_owned_checkout: code=$code out=$out"; fail=1
else
  echo "PASS local_owned_checkout"
fi

for checkout in dependency nonmember foreign-root foreign-target untracked metadata-failure; do
  out=$(run_installer local callisto@0.5.0 "$checkout"); code=$?
  if [[ $code -eq 0 || "$out" != *"source=local requires a Callisto workspace checkout"* \
    || "$out" == *"cargo build"* || "$out" == *"cargo install"* || "$out" == *"curl "* \
    || ( "$checkout" != untracked && "$out" != *"cargo metadata "* ) ]]; then
    echo "FAIL local_rejects_$checkout: code=$code out=$out"; fail=1
  else
    echo "PASS local_rejects_$checkout"
  fi
done

for action_ref in '' 0000000000000000000000000000000000000f; do
  out=$(run_installer auto latest dependency "$action_ref"); code=$?
  if [[ $code -ne 0 || "$out" != *"cargo install callisto-cli --locked "* \
    || "$out" == *"--version"* || "$out" == *"cargo build"* || "$out" == *"cargo metadata"* ]]; then
    echo "FAIL auto_latest_consumer_${action_ref:-unset}: code=$code out=$out"; fail=1
  else
    echo "PASS auto_latest_consumer_${action_ref:-unset}"
  fi
done

out=$(run_installer local callisto@0.5.0 owned); code=$?
if [[ $code -ne 0 || "$out" != *"cargo build --locked --manifest-path "* \
  || "$out" == *"cargo install"* || "$out" == *"curl "* ]]; then
  echo "FAIL local_explicit_pin_builds_checkout: code=$code out=$out"; fail=1
else
  echo "PASS local_explicit_pin_builds_checkout"
fi

exit "$fail"
