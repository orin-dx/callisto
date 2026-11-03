#!/usr/bin/env bash
# Executes a typed Callisto release-PR decision. Forge reads stay here; all
# create/update policy belongs to `callisto release-pr decide`.
#
# The managed branch is never pushed with the Git wire protocol: on a public
# repository, GITHUB_TOKEN cannot write `.github/workflows/*` that way, nor
# through createCommitOnBranch's own fileChanges, on any ref (confirmed
# directly against this repository). Instead, the generated
# version-bump changes are committed via the forge commit API onto a
# deterministic staging branch rooted at the current base commit -- which
# never needs to touch `.github/workflows/*`, since that path is inherited
# unchanged from the base -- and only then is the managed branch's ref moved
# onto that commit. A ref move is not itself a write to that path.
set -euo pipefail

cd "$INPUT_CWD"

# This checkout starts on the configured base. Preserve its exact commit for
# both observations; the working tree changes below (git add -A) never
# create a local commit -- the real commit is made remotely, via the forge
# commit API, on the staging branch.
base_commit=$(git rev-parse HEAD)

# One GitHub 5xx must not strand the release PR until the next push to main: retry transient failures,
# print every failed attempt's stderr, and leave other failures (404, 422) to the caller. `gh pr create`
# is not wrapped, because a retry after an unseen success would fail on the PR it just made.
gh_retry() {
  local attempt=1 delay=2 err code
  err=$(mktemp)
  until gh "$@" 2> "$err"; do
    code=$?
    cat "$err" >&2
    if [[ $attempt -ge 4 ]] || ! grep -qiE 'Something went wrong|HTTP 5[0-9]{2}|Bad Gateway|Service Unavailable|Gateway Time|timed? ?out|connection reset|unexpected EOF' "$err"; then
      rm -f "$err"
      return "$code"
    fi
    echo "::warning::gh $1 hit a transient GitHub error (attempt $attempt of 4); retrying in ${delay}s" >&2
    sleep "$delay"
    delay=$((delay * 2))
    attempt=$((attempt + 1))
    [[ -z "${RETRY_RESET:-}" ]] || "$RETRY_RESET" || true
  done
  cat "$err" >&2
  rm -f "$err"
}

release_pr_snapshot() {
  local raw_prs
  # Command substitutions do not inherit set -e, so a failed read must stop here, not later in jq.
  raw_prs=$(gh_retry pr list --state open --base "$INPUT_BRANCH" --limit 1000 --json number,headRefName,headRepository,headRefOid) || return $?
  jq -cn \
    --arg repository "$GITHUB_REPOSITORY" \
    --arg base_branch "$INPUT_BRANCH" \
    --arg base_commit "$base_commit" \
    --argjson raw_prs "$raw_prs" \
    '{
      schemaVersion: 2,
      repository: $repository,
      baseBranch: $base_branch,
      baseCommit: $base_commit,
      openPullRequests: [$raw_prs[] | {
        number,
        headRepository: (.headRepository.nameWithOwner // ""),
        headBranch: .headRefName,
        headCommit: .headRefOid
      }]
    }'
}

# Reads the open PRs first, so a failed read stops with `gh`'s own status instead of becoming an empty --snapshot.
verify_current() {
  local current
  current=$(release_pr_snapshot) || return $?
  callisto release-pr verify --decision "$decision" --snapshot "$current" "$@"
}

snapshot=$(release_pr_snapshot)
decision=$(callisto --format json release-pr decide \
  --snapshot "$snapshot" \
  --repository "$GITHUB_REPOSITORY" \
  --base-branch "$INPUT_BRANCH" \
  --release-branch "$INPUT_RELEASE_BRANCH")
decision_kind=$(jq -r '.action.kind' <<< "$decision")

# A missing key must stop the run: `jq -r` would print `null`, and the script would then move a branch named null.
decision_field() {
  jq -er "$1" <<< "$decision" || { echo "::error::Callisto's release PR decision has no $1; refusing to guess" >&2; return 1; }
}

case "$decision_kind" in
  noop)
    echo 'hasChangesets=false' >> "$GITHUB_OUTPUT"
    echo '::notice::No changesets. This action does not publish, tag, download artifacts, or create releases; use the durable repository workflow.'
    exit 0
    ;;
  create)
    release_branch=$(decision_field '.action.branch')
    staging_branch=$(decision_field '.action.stagingBranch')
    existing_pr=''
    ;;
  update)
    existing_pr=$(decision_field '.action.pullRequestNumber')
    release_branch=$(decision_field '.action.branch')
    staging_branch=$(decision_field '.action.stagingBranch')
    ;;
  *) echo "::error::Callisto returned unsupported release PR decision kind: $decision_kind"; exit 1 ;;
esac

echo 'hasChangesets=true' >> "$GITHUB_OUTPUT"
if [[ -n "$existing_pr" ]]; then
  existing_body=$(gh_retry pr view "$existing_pr" --json body --jq '.body')
  body=$(printf '%s' "$existing_body" | callisto compose-pr-body --existing-body - --branch "$INPUT_BRANCH" --format text)
else
  body=$(callisto compose-pr-body --branch "$INPUT_BRANCH" --format text)
fi

read -ra command <<< "$INPUT_VERSION_COMMAND"
command+=(--emit-decision "$INPUT_DECISION_PATH")
"${command[@]}"
git add -A
test -n "$(git status --porcelain)" || { echo '::error::pending changesets produced no release-PR delta'; exit 1; }

plan_file=$(mktemp)
callisto --format json release-pr commit-plan --base-commit "$base_commit" --message "$INPUT_COMMIT_MESSAGE" --out "$plan_file"
local_tree=$(git write-tree)

staging_created=false
cleanup_staging() {
  if [[ "$staging_created" == true ]]; then
    gh api -X DELETE "repos/$GITHUB_REPOSITORY/git/refs/heads/$staging_branch" > /dev/null 2>&1 \
      || echo "::warning::could not delete staging branch $staging_branch; the next run reclaims this deterministic name"
  fi
  rm -f "$plan_file" body.json
}
trap cleanup_staging EXIT

# A local diff does not authorize a stale forge write. Re-observe immediately
# before mutating the remote branch, then again before moving the managed
# ref, then again before GitHub label/PR writes.
verify_current > /dev/null

reset_staging() {
  gh_retry api -X PATCH "repos/$GITHUB_REPOSITORY/git/refs/heads/$staging_branch" -f sha="$base_commit" -F force=true > /dev/null 2>&1
}

if reset_staging; then
  :
else
  gh_retry api -X POST "repos/$GITHUB_REPOSITORY/git/refs" -f ref="refs/heads/$staging_branch" -f sha="$base_commit" > /dev/null
fi
staging_created=true

jq -n \
  --arg repo "$GITHUB_REPOSITORY" \
  --arg branch "$staging_branch" \
  --arg headline "$INPUT_COMMIT_MESSAGE" \
  --slurpfile plan "$plan_file" \
  '{
    query: "mutation($input: CreateCommitOnBranchInput!) { createCommitOnBranch(input: $input) { commit { oid } } }",
    variables: {
      input: {
        branch: {repositoryNameWithOwner: $repo, branchName: $branch},
        message: {headline: $headline},
        expectedHeadOid: $plan[0].baseCommit,
        fileChanges: {
          additions: [$plan[0].additions[] | {path, contents: .contentsBase64}],
          deletions: $plan[0].deletions
        }
      }
    }
  }' > body.json

# A retry may follow a commit the forge made but never acknowledged, which would fail the mutation's
# expectedHeadOid, so each retry first puts the staging branch back on the base commit.
new_sha=$(RETRY_RESET=reset_staging gh_retry api graphql --input body.json --jq '.data.createCommitOnBranch.commit.oid')
remote_tree=$(gh_retry api "repos/$GITHUB_REPOSITORY/git/commits/$new_sha" --jq '.tree.sha')
if [[ "$remote_tree" != "$local_tree" ]]; then
  echo "::error::forge commit tree $remote_tree does not match the locally staged tree $local_tree; refusing to move $release_branch"
  exit 1
fi

verify_current > /dev/null

if [[ "$decision_kind" == update ]]; then
  gh_retry api -X PATCH "repos/$GITHUB_REPOSITORY/git/refs/heads/$release_branch" -f sha="$new_sha" -F force=true > /dev/null
else
  if ! gh_retry api -X POST "repos/$GITHUB_REPOSITORY/git/refs" -f ref="refs/heads/$release_branch" -f sha="$new_sha" > /dev/null 2>&1; then
    gh_retry api -X PATCH "repos/$GITHUB_REPOSITORY/git/refs/heads/$release_branch" -f sha="$new_sha" -F force=true > /dev/null
  fi
fi
moved_sha=$(gh_retry api "repos/$GITHUB_REPOSITORY/git/refs/heads/$release_branch" --jq '.object.sha')
if [[ "$moved_sha" != "$new_sha" ]]; then
  echo "::error::$release_branch points at $moved_sha after the update, expected $new_sha; a concurrent write may have raced this run"
  exit 1
fi

if [[ "$decision_kind" == update ]]; then
  # The PR list can lag a ref move by a moment, and a stale read looks like a changed snapshot.
  # A real concurrent change stays visible, so the last attempt still fails.
  verified=false
  for _ in 1 2 3 4 5 6 7 8 9; do
    if verify_current --applied-head "$new_sha" > /dev/null 2>&1; then
      verified=true
      break
    fi
    sleep 3
  done
  if [[ "$verified" != true ]]; then
    verify_current --applied-head "$new_sha" > /dev/null
  fi
else
  verify_current > /dev/null
fi

label_exists=$(gh_retry label list --limit 1000 --json name --jq 'any(.[]; .name == env.INPUT_PR_LABEL)')
if [[ "$label_exists" != true ]]; then
  gh_retry label create "$INPUT_PR_LABEL" --color 0e8a16 --description 'Callisto release packages'
fi

case "$decision_kind" in
  create)
    # Born a draft, so a failed tip check below leaves it unmergeable rather than ready on a stale base.
    create_args=(--draft --head "$release_branch" --base "$INPUT_BRANCH" --title "$INPUT_TITLE" --body "$body" --label "$INPUT_PR_LABEL")
    pr_url=$(gh pr create "${create_args[@]}")
    echo "$pr_url"
    pr_number=${pr_url##*/}
    ;;
  update)
    edit_args=(--title "$INPUT_TITLE" --body "$body" --add-label "$INPUT_PR_LABEL")
    gh_retry pr edit "$existing_pr" "${edit_args[@]}"
    pr_number=$existing_pr
    ;;
esac

# The hold workflow drafts the release PR whenever main moves, because a draft cannot be merged. Hand it back only
# when it was built from the current tip; if main moved again during this run, it stays a draft for the next run.
is_draft=$(gh_retry pr view "$pr_number" --json isDraft --jq '.isDraft')
tip=$(gh_retry api "repos/$GITHUB_REPOSITORY/git/ref/heads/$INPUT_BRANCH" --jq '.object.sha')
if [[ "$tip" == "$base_commit" ]]; then
  if [[ "$is_draft" == true ]]; then gh_retry pr ready "$pr_number"; fi
else
  echo "::warning::$INPUT_BRANCH moved to $tip while this run built the release PR from $base_commit; leaving it a draft for the next run"
  if [[ "$is_draft" != true ]]; then gh_retry pr ready "$pr_number" --undo; fi
fi
