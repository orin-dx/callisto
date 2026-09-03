#!/usr/bin/env bash
# Executes a typed Callisto release-PR decision. Forge reads stay here; all
# create/update/supersede policy belongs to `callisto release-pr decide`.
set -euo pipefail

cd "$INPUT_CWD"

# This checkout starts on the configured base. Preserve its exact commit for
# both observations; after `version` commits, HEAD intentionally changes.
base_commit=$(git rev-parse HEAD)

release_pr_snapshot() {
  local raw_prs initial enriched number repository branch workflow_delta
  raw_prs=$(gh pr list --state open --base "$INPUT_BRANCH" --limit 1000 --json number,headRefName,headRepository)
  initial=$(jq -c '[.[] | {
    number,
    headRepository: (.headRepository.nameWithOwner // ""),
    headBranch: .headRefName,
    workflowDeltaFromBase: false
  }]' <<< "$raw_prs")
  enriched='[]'

  # This is forge/Git observation, not policy: fetch every safe same-repository
  # PR branch and report whether its workflow files differ from the checked-out
  # base. Callisto decides whether that fact requires a replacement PR.
  while IFS=$'\t' read -r number repository branch; do
    workflow_delta=false
    if [[ "$repository" == "$GITHUB_REPOSITORY" ]] && git check-ref-format --branch "$branch" > /dev/null 2>&1; then
      git fetch --no-tags origin "refs/heads/$branch:refs/remotes/origin/$branch"
      if ! git diff --quiet "refs/remotes/origin/$branch...HEAD" -- .github/workflows; then
        workflow_delta=true
      fi
    fi
    enriched=$(jq -c \
      --argjson number "$number" \
      --arg repository "$repository" \
      --arg branch "$branch" \
      --argjson workflow_delta "$workflow_delta" \
      '. + [{number: $number, headRepository: $repository, headBranch: $branch, workflowDeltaFromBase: $workflow_delta}]' \
      <<< "$enriched")
  done < <(jq -r '.[] | [.number, .headRepository, .headBranch] | @tsv' <<< "$initial")

  jq -cn \
    --arg repository "$GITHUB_REPOSITORY" \
    --arg base_branch "$INPUT_BRANCH" \
    --arg base_commit "$base_commit" \
    --argjson open_pull_requests "$enriched" \
    '{schemaVersion: 1, repository: $repository, baseBranch: $base_branch, baseCommit: $base_commit, openPullRequests: $open_pull_requests}'
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
    existing_pr=''
    ;;
  update)
    existing_pr=$(jq -r '.action.pullRequestNumber' <<< "$decision")
    release_branch=$(jq -r '.action.branch' <<< "$decision")
    ;;
  supersede)
    existing_pr=$(jq -r '.action.pullRequestNumber' <<< "$decision")
    release_branch=$(jq -r '.action.replacementBranch' <<< "$decision")
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
test -n "$(git status --porcelain)" || { echo '::error::pending changesets produced no release-PR delta'; exit 1; }
if [[ "$INPUT_SETUP_GIT_USER" == true ]]; then
  git config user.name 'github-actions[bot]'
  git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
fi
git switch -C "$release_branch"
git add -A
git commit -m "$INPUT_COMMIT_MESSAGE"

# A local commit does not authorize a stale force-push. Re-observe immediately
# before mutating the remote branch, then again before GitHub label/PR writes.
current_snapshot=$(release_pr_snapshot)
callisto release-pr verify --decision "$decision" --snapshot "$current_snapshot" > /dev/null
git push --force-with-lease origin "$release_branch"

current_snapshot=$(release_pr_snapshot)
callisto release-pr verify --decision "$decision" --snapshot "$current_snapshot" > /dev/null

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
  supersede)
    new_pr_url=$(gh pr create --head "$release_branch" --base "$INPUT_BRANCH" --title "$INPUT_TITLE" --body "$body" --label "$INPUT_PR_LABEL")
    gh pr close "$existing_pr" --comment "Superseded by $new_pr_url because GitHub requires elevated workflow authority to update this generated branch. The replacement was recomputed from the current base branch and pending changesets."
    ;;
esac
