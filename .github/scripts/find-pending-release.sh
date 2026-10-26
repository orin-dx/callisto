#!/usr/bin/env bash
# The oldest merged managed PR still awaiting verified release work.
set -euo pipefail

repo="$1"
base="$2"
label="$3"
exclude="${4:-}"
branch='callisto/version-packages'
if [[ -n "$exclude" && ! "$exclude" =~ ^[0-9a-f]{40}$ ]]; then
  echo 'excluded source SHA must be 40 lowercase hex characters' >&2
  exit 1
fi
prs=$(gh pr list --state merged --base "$base" --label "$label" --repo "$repo" --limit 1001 \
  --json number,mergedAt,mergeCommit,headRefName,headRepository,baseRefName,labels)
jq -r --arg repo "$repo" --arg base "$base" --arg branch "$branch" --arg exclude "$exclude" '
  if type != "array" then error("merged PR list is not an array")
  elif length >= 1001 then error("too many pending release PRs to select safely")
  else
    [.[]
      | select((.baseRefName // "") == $base and (.headRepository.nameWithOwner // "") == $repo)
      | select(any(.labels[]?; .name == "callisto: complete") | not)
      | select((.headRefName // "") as $head
          | $head == $branch or (($head | startswith($branch + "--"))
            and ($head | ltrimstr($branch + "--") | test("^[0-9a-f]{40}$"))))
    ] as $managed
    | if any($managed[]; (.mergeCommit.oid // "") | test("^[0-9a-f]{40}$") | not)
      then error("a pending managed release PR has no full merge commit")
      else [$managed[] | select(.mergeCommit.oid != $exclude)] | sort_by(.mergedAt, .number) | .[0].mergeCommit.oid // empty
      end
  end
' <<< "$prs"
