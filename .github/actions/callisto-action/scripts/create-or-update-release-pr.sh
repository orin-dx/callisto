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

release_pr_snapshot() {
  local raw_prs
  raw_prs=$(gh pr list --state open --base "$INPUT_BRANCH" --limit 1000 --json number,headRefName,headRepository,headRefOid)
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

snapshot=$(release_pr_snapshot)
decision=$(callisto --format json release-pr decide \
  --snapshot "$snapshot" \
  --repository "$GITHUB_REPOSITORY" \
  --base-branch "$INPUT_BRANCH" \
  --release-branch "$INPUT_RELEASE_BRANCH")
decision_kind=$(jq -r '.action.kind' <<< "$decision")

case "$decision_kind" in
  noop)
    echo 'hasChangesets=false' >> "$GITHUB_OUTPUT"
    matrix=$(callisto matrix --format json | jq -c '[.platformTargets[].targets[]] | unique_by(.artifactName)')
    echo "nativeMatrix=$matrix" >> "$GITHUB_OUTPUT"
    echo '::notice::No changesets. This action does not publish, tag, download artifacts, or create releases; use the durable repository workflow.'
    exit 0
    ;;
  create)
    release_branch=$(jq -r '.action.branch' <<< "$decision")
    staging_branch=$(jq -r '.action.stagingBranch' <<< "$decision")
    existing_pr=''
    ;;
  update)
    existing_pr=$(jq -r '.action.pullRequestNumber' <<< "$decision")
    release_branch=$(jq -r '.action.branch' <<< "$decision")
    staging_branch=$(jq -r '.action.stagingBranch' <<< "$decision")
    ;;
  *) echo "::error::Callisto returned unsupported release PR decision kind: $decision_kind"; exit 1 ;;
esac

echo 'hasChangesets=true' >> "$GITHUB_OUTPUT"
if [[ -n "$existing_pr" ]]; then
  existing_body=$(gh pr view "$existing_pr" --json body --jq '.body')
  body=$(printf '%s' "$existing_body" | callisto compose-pr-body --existing-body - --label "$INPUT_PR_LABEL" --branch "$INPUT_BRANCH" --format text)
else
  body=$(callisto compose-pr-body --label "$INPUT_PR_LABEL" --branch "$INPUT_BRANCH" --format text)
fi

read -ra command <<< "$INPUT_VERSION_COMMAND"
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
callisto release-pr verify --decision "$decision" --snapshot "$(release_pr_snapshot)" > /dev/null

if gh api -X PATCH "repos/$GITHUB_REPOSITORY/git/refs/heads/$staging_branch" -f sha="$base_commit" -F force=true > /dev/null 2>&1; then
  :
else
  gh api -X POST "repos/$GITHUB_REPOSITORY/git/refs" -f ref="refs/heads/$staging_branch" -f sha="$base_commit" > /dev/null
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

new_sha=$(gh api graphql --input body.json --jq '.data.createCommitOnBranch.commit.oid')
remote_tree=$(gh api "repos/$GITHUB_REPOSITORY/git/commits/$new_sha" --jq '.tree.sha')
if [[ "$remote_tree" != "$local_tree" ]]; then
  echo "::error::forge commit tree $remote_tree does not match the locally staged tree $local_tree; refusing to move $release_branch"
  exit 1
fi

callisto release-pr verify --decision "$decision" --snapshot "$(release_pr_snapshot)" > /dev/null

if [[ "$decision_kind" == update ]]; then
  gh api -X PATCH "repos/$GITHUB_REPOSITORY/git/refs/heads/$release_branch" -f sha="$new_sha" -F force=true > /dev/null
else
  if ! gh api -X POST "repos/$GITHUB_REPOSITORY/git/refs" -f ref="refs/heads/$release_branch" -f sha="$new_sha" > /dev/null 2>&1; then
    gh api -X PATCH "repos/$GITHUB_REPOSITORY/git/refs/heads/$release_branch" -f sha="$new_sha" -F force=true > /dev/null
  fi
fi
moved_sha=$(gh api "repos/$GITHUB_REPOSITORY/git/refs/heads/$release_branch" --jq '.object.sha')
if [[ "$moved_sha" != "$new_sha" ]]; then
  echo "::error::$release_branch points at $moved_sha after the update, expected $new_sha; a concurrent write may have raced this run"
  exit 1
fi

callisto release-pr verify --decision "$decision" --snapshot "$(release_pr_snapshot)" > /dev/null

label_exists=$(gh label list --limit 1000 --json name --jq 'any(.[]; .name == env.INPUT_PR_LABEL)')
if [[ "$label_exists" != true ]]; then
  gh label create "$INPUT_PR_LABEL" --color 0e8a16 --description 'Callisto release packages'
fi

case "$decision_kind" in
  create)
    gh pr create --head "$release_branch" --base "$INPUT_BRANCH" --title "$INPUT_TITLE" --body "$body" --label "$INPUT_PR_LABEL"
    ;;
  update)
    gh pr edit "$existing_pr" --title "$INPUT_TITLE" --body "$body" --add-label "$INPUT_PR_LABEL"
    ;;
esac
