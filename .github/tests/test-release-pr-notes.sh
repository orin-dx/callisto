#!/usr/bin/env bash
# Behavior test for the release PR state comment (release-pr-note.sh) and the rebuild note (note-release-pr-rebuild.sh), with a stubbed gh.
set -u
scripts="$(cd "$(dirname "${BASH_SOURCE[0]}")/../scripts" && pwd)"
fail=0

run_case() { # <script> <pr list json> <existing comment ids> <script args...>
  local script="$1" pr_list="$2" comment_ids="$3"
  shift 3
  local calls harness
  calls="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  harness="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  cat > "$harness" <<'STUBS'
gh() {
  echo "gh $*" >> "$CALLS_FILE"
  case "$1" in
    pr) printf '%s\n' "$PR_LIST" ;;
    api)
      if [[ " $* " == *'/commits/main '* ]]; then
        [[ -z "$MAIN_TIP_FAILS" ]] || return 1
        printf '%s\n' "$MAIN_TIP"
      elif [[ " $* " == *'/comments '* && " $* " != *' -X '* ]]; then
        [[ -z "$LOOKUP_FAILS" ]] || return 1
        local filter='' previous='' argument
        for argument in "$@"; do
          [[ "$previous" == --jq ]] && filter="$argument"
          previous="$argument"
        done
        jq -r "$filter" <<< "$COMMENTS_JSON"
      fi ;;
  esac
}
STUBS
  # The comments the PR already has: someone else's, plus the state comment when an id is given.
  local comments='[{"id":9,"body":"an unrelated comment"}]'
  if [[ -n "$comment_ids" ]]; then
    comments="[{\"id\":9,\"body\":\"an unrelated comment\"},{\"id\":$comment_ids,\"body\":\"<!-- callisto-release-pr-state -->\\nold\"}]"
  fi
  CALLS_FILE="$calls" PR_LIST="$pr_list" COMMENTS_JSON="$comments" LOOKUP_FAILS="${LOOKUP_FAILS:-}" GITHUB_REPOSITORY='orin-dx/callisto' \
    MAIN_TIP="${MAIN_TIP:-0123456789abcdef0123456789abcdef01234567}" MAIN_TIP_FAILS="${MAIN_TIP_FAILS:-}" \
    GITHUB_SHA='0123456789abcdef0123456789abcdef01234567' \
    bash -c 'source "$1"; script=$2; shift 2; source "$script" "$@"; if [[ "$script" == */release-pr-note.sh ]]; then release_pr_note "$@"; fi' \
    -- "$harness" "$scripts/$script" "$@" > /dev/null 2>&1
  local code=$?
  printf '%s\n---calls---\n' "$code"
  grep -v 'api --paginate' "$calls" | cut -c1-400
  rm -f "$calls" "$harness"
}

pr() { # <number> <branch> <draft>
  printf '[{"number":%s,"headRefName":"%s","isDraft":%s,"headRepository":{"nameWithOwner":"orin-dx/callisto"}}]' "$1" "$2" "$3"
}
verdict() { if [[ "$2" == 0 ]]; then echo "PASS: $1"; else echo "FAIL: $1"; fail=1; fi; }
ok=1

# The state comment is created once and then edited in place.
output="$(run_case release-pr-note.sh '[]' '' held 7)"
if [[ "$output" == 0$'\n'* && "$output" == *'-X POST repos/orin-dx/callisto/issues/7/comments'* && "$output" == *'Held as a draft'* && "$output" == *'0123456'* ]]; then ok=0; else ok=1; fi
verdict 'a first note creates the comment' "$ok"

output="$(run_case release-pr-note.sh '[]' '555' ready 7)"
if [[ "$output" == 0$'\n'* && "$output" == *'-X PATCH repos/orin-dx/callisto/issues/comments/555'* && "$output" != *'-X POST'* && "$output" == *'Ready: rebuilt'* ]]; then ok=0; else ok=1; fi
verdict 'a later note edits the same comment' "$ok"

output="$(run_case release-pr-note.sh '[]' '' failed 7 https://example.test/run/9)"
if [[ "$output" == 0$'\n'* && "$output" == *'https://example.test/run/9'* && "$output" == *'Still a draft'* ]]; then ok=0; else ok=1; fi
verdict 'a failed note links the run' "$ok"

output="$(run_case release-pr-note.sh '[]' '' nonsense 7)"
if [[ "$output" == 2$'\n'* && "$output" != *'POST'* ]]; then ok=0; else ok=1; fi
verdict 'an unknown state writes nothing and fails' "$ok"

# The rebuild note: ready only for a PR that is not a draft, failed with the run link, nothing without a release PR.
output="$(run_case note-release-pr-rebuild.sh "$(pr 7 callisto/version-packages false)" '' callisto/version-packages success https://example.test/run/9)"
if [[ "$output" == 0$'\n'* && "$output" == *'Ready: rebuilt'* ]]; then ok=0; else ok=1; fi
verdict 'a successful rebuild of a ready PR says ready' "$ok"

output="$(run_case note-release-pr-rebuild.sh "$(pr 7 callisto/version-packages true)" '' callisto/version-packages success https://example.test/run/9)"
if [[ "$output" == 0$'\n'* && "$output" != *'POST'* && "$output" != *'PATCH'* ]]; then ok=0; else ok=1; fi
verdict 'a successful rebuild that left the PR a draft leaves the hold note' "$ok"

output="$(run_case note-release-pr-rebuild.sh "$(pr 7 callisto/version-packages true)" '' callisto/version-packages failure https://example.test/run/9)"
if [[ "$output" == 0$'\n'* && "$output" == *'Still a draft'* && "$output" == *'https://example.test/run/9'* ]]; then ok=0; else ok=1; fi
verdict 'a failed rebuild says so and links the run' "$ok"

output="$(run_case note-release-pr-rebuild.sh '[]' '' callisto/version-packages failure https://example.test/run/9)"
if [[ "$output" == 0$'\n'* && "$output" != *'POST'* ]]; then ok=0; else ok=1; fi
verdict 'no release PR means no note' "$ok"

# A rebuild of a commit that is no longer the tip of main leaves the newer push's note alone; an unreadable tip writes nothing.
output="$(MAIN_TIP=fedcba9876543210fedcba9876543210fedcba98 run_case note-release-pr-rebuild.sh "$(pr 7 callisto/version-packages true)" '' callisto/version-packages failure https://example.test/run/9)"
if [[ "$output" == 0$'\n'* && "$output" != *'POST'* && "$output" != *'PATCH'* ]]; then ok=0; else ok=1; fi
verdict 'a failed rebuild of a superseded commit leaves the note alone' "$ok"

output="$(MAIN_TIP=fedcba9876543210fedcba9876543210fedcba98 run_case note-release-pr-rebuild.sh "$(pr 7 callisto/version-packages false)" '' callisto/version-packages success https://example.test/run/9)"
if [[ "$output" == 0$'\n'* && "$output" != *'POST'* && "$output" != *'PATCH'* ]]; then ok=0; else ok=1; fi
verdict 'a successful rebuild of a superseded commit leaves the note alone' "$ok"

output="$(MAIN_TIP_FAILS=1 run_case note-release-pr-rebuild.sh "$(pr 7 callisto/version-packages true)" '' callisto/version-packages failure https://example.test/run/9)"
if [[ "$output" == 0$'\n'* && "$output" != *'POST'* && "$output" != *'PATCH'* ]]; then ok=0; else ok=1; fi
verdict 'an unreadable main tip writes nothing and only warns' "$ok"

# The lookup finds the state comment by its marker and ignores other people's comments.
output="$(run_case release-pr-note.sh '[]' '555' held 7)"
if [[ "$output" == 0$'\n'* && "$output" == *'issues/comments/555'* && "$output" != *'issues/comments/9'* ]]; then ok=0; else ok=1; fi
verdict 'the state comment is found by its marker, not by position' "$ok"

# A failed lookup must not read as "no comment yet": no second comment, and the failure is returned.
output="$(LOOKUP_FAILS=1 run_case release-pr-note.sh '[]' '555' held 7)"
if [[ "$output" == 1$'\n'* && "$output" != *'-X POST'* && "$output" != *'-X PATCH'* ]]; then ok=0; else ok=1; fi
verdict 'a failed comment lookup writes nothing and fails' "$ok"

# A rebuild that was skipped upstream (nothing ran) writes nothing; an outcome nobody defined is an error.
output="$(run_case note-release-pr-rebuild.sh "$(pr 7 callisto/version-packages true)" '' callisto/version-packages skipped https://example.test/run/9)"
if [[ "$output" == 0$'\n'* && "$output" != *'POST'* && "$output" != *'PATCH'* ]]; then ok=0; else ok=1; fi
verdict 'a skipped rebuild writes nothing' "$ok"

output="$(run_case note-release-pr-rebuild.sh "$(pr 7 callisto/version-packages true)" '' callisto/version-packages bogus https://example.test/run/9)"
if [[ "$output" == 2$'\n'* && "$output" != *'POST'* ]]; then ok=0; else ok=1; fi
verdict 'an unknown outcome fails instead of being ignored' "$ok"

exit "$fail"
