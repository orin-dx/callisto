#!/usr/bin/env bash
# Behavior test for .github/scripts/hold-release-pr.sh with a stubbed gh.
set -u
script="$(cd "$(dirname "${BASH_SOURCE[0]}")/../scripts" && pwd)/hold-release-pr.sh"
fail=0

run_case() { # <pr list json> [failing ready attempts]
  local calls harness count
  calls="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  harness="$(mktemp "${TMPDIR:-/tmp}/callisto.XXXXXX")"
  count="$calls.count"
  cat > "$harness" <<'STUBS'
gh() {
  echo "gh $*" >> "$CALLS_FILE"
  case "$1 $2" in
    'pr list') printf '%s\n' "$PR_LIST" ;;
    'pr ready')
      local seen=$(( $(cat "$READY_COUNT_FILE" 2>/dev/null || echo 0) + 1 ))
      echo "$seen" > "$READY_COUNT_FILE"
      [[ "$seen" -gt "${READY_FAILS:-0}" ]] || return 1 ;;
  esac
}
sleep() { echo "sleep $*" >> "$CALLS_FILE"; }
STUBS
  CALLS_FILE="$calls" PR_LIST="$1" READY_FAILS="${2:-0}" READY_COUNT_FILE="$count" GITHUB_REPOSITORY='orin-dx/callisto' \
    bash -c 'source "$1"; source "$2" callisto/version-packages' -- "$harness" "$script" > /dev/null 2>&1
  local code=$?
  printf '%s\n---calls---\n' "$code"
  cat "$calls"
  rm -f "$calls" "$harness" "$count"
}

pr() { # <number> <branch> <draft> [repo]
  printf '{"number":%s,"headRefName":"%s","isDraft":%s,"headRepository":{"nameWithOwner":"%s"}}' "$1" "$2" "$3" "${4:-orin-dx/callisto}"
}
verdict() { # <name> <0 when it holds>
  if [[ "$2" == 0 ]]; then echo "PASS: $1"; else echo "FAIL: $1"; fail=1; fi
}

sha=0123456789abcdef0123456789abcdef01234567
ok=1

output="$(run_case "[$(pr 7 callisto/version-packages false)]")"
if [[ "$output" == 0$'\n'* && "$output" == *'gh pr ready 7 --repo orin-dx/callisto --undo'* ]]; then ok=0; else ok=1; fi
verdict 'a ready release PR is drafted' "$ok"

output="$(run_case "[$(pr 8 "callisto/version-packages--$sha" false)]")"
if [[ "$output" == 0$'\n'* && "$output" == *'gh pr ready 8 '* ]]; then ok=0; else ok=1; fi
verdict 'a SHA-suffixed replacement release PR is drafted' "$ok"

output="$(run_case "[$(pr 9 callisto/version-packages true)]")"
if [[ "$output" == 0$'\n'* && "$output" != *'gh pr ready'* ]]; then ok=0; else ok=1; fi
verdict 'an already-draft release PR is left alone' "$ok"

output="$(run_case "[$(pr 10 feature/x false),$(pr 11 callisto/version-packages false other/fork)]")"
if [[ "$output" == 0$'\n'* && "$output" != *'gh pr ready'* ]]; then ok=0; else ok=1; fi
verdict 'other PRs and fork PRs are left alone' "$ok"

output="$(run_case '[]')"
if [[ "$output" == 0$'\n'* && "$output" != *'gh pr ready'* ]]; then ok=0; else ok=1; fi
verdict 'no release PR means no write' "$ok"

output="$(run_case "[$(pr 7 callisto/version-packages false)]" 2)"
if [[ "$output" == 0$'\n'* && "$(grep -c 'gh pr ready' <<< "$output")" == 3 ]]; then ok=0; else ok=1; fi
verdict 'a failed draft call is retried' "$ok"

output="$(run_case "[$(pr 7 callisto/version-packages false)]" 100)"
if [[ "$output" == 1$'\n'* && "$(grep -c 'gh pr ready' <<< "$output")" == 3 ]]; then ok=0; else ok=1; fi
verdict 'a draft call that never succeeds fails the job after three attempts' "$ok"

exit "$fail"
