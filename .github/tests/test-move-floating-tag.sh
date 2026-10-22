#!/usr/bin/env bash
# Behavior test for .github/scripts/move-floating-tag.sh against a real local remote.
set -u
SCRIPT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../scripts" && pwd)/move-floating-tag.sh"
fail=0

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t

git init -q --bare "$tmp/remote.git"
git clone -q "$tmp/remote.git" "$tmp/work" 2>/dev/null
cd "$tmp/work" || exit 2
git config commit.gpgsign false
commit() {
  echo "$1" > file
  git add file
  git commit -q -m "$1"
  git rev-parse HEAD
}
c1=$(commit one)
c2=$(commit two)
c3=$(commit three)
git push -q origin HEAD:refs/heads/main

decision() { # <package> <version>
  printf '{"schemaVersion":1,"entries":[{"package":"%s","targetVersion":"%s","reasons":[]}]}\n' "$1" "$2" > "$tmp/decision.json"
}
remote_tag() { git ls-remote --tags origin "refs/tags/$1" | awk '{print $1}'; }
check() { # <name> <expected> <actual>
  if [[ "$2" == "$3" ]]; then echo "PASS $1"; else echo "FAIL $1: expected '$2', got '$3'"; fail=1; fi
}
run() { bash "$SCRIPT" "$tmp/decision.json" cargo/callisto-cli "$1" origin >"$tmp/out" 2>&1; }

decision cargo/callisto-cli 0.12.0
run "$c1"
check "before 1.0.0 no tag" "" "$(remote_tag v0)$(remote_tag v1)"

decision cargo/callisto-cli 1.0.0
run "$c1"
check "1.0.0 creates v1" "$c1" "$(remote_tag v1)"

decision cargo/callisto-cli 1.1.0
run "$c3"
check "1.1.0 moves v1 forward" "$c3" "$(remote_tag v1)"

decision cargo/callisto-cli 1.0.1
run "$c2"
check "an older commit never moves v1 backwards" "$c3" "$(remote_tag v1)"

decision cargo/callisto-cli 1.1.0
run "$c3"
check "rerun on the same commit is a no-op" "$c3" "$(remote_tag v1)"

decision cargo/callisto-cli 1.2.0-beta.1
run "$c3"
check "a prerelease moves nothing" "" "$(remote_tag v1 | grep -v "^$c3\$")"

decision cargo/other 1.5.0
run "$c3"
check "a release without the product package moves nothing" "" "$(remote_tag v1 | grep -v "^$c3\$")"

decision cargo/callisto-cli 2.0.0
run "$c3"
check "2.0.0 creates v2 and leaves v1" "$c3 $c3" "$(remote_tag v2) $(remote_tag v1)"

decision cargo/callisto-cli 1.1.0
if bash "$SCRIPT" "$tmp/decision.json" cargo/callisto-cli notasha origin >"$tmp/out" 2>&1; then
  echo "FAIL a malformed commit is refused"; fail=1
else
  echo "PASS a malformed commit is refused"
fi

exit "$fail"
