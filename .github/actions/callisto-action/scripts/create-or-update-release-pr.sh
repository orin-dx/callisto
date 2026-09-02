#!/usr/bin/env bash
# Creates or refreshes the non-privileged Callisto release PR. This script is
# intentionally the composite action's executable implementation so its tests
# exercise the exact code a GitHub runner invokes.
set -euo pipefail

cd "$INPUT_CWD"

set +e
callisto status --check
status=$?
set -e
case "$status" in
  1) echo 'hasChangesets=true' >> "$GITHUB_OUTPUT" ;;
  2)
    echo 'hasChangesets=false' >> "$GITHUB_OUTPUT"
    matrix=$(callisto matrix --format json | jq -c '[.platformTargets[].targets[]] | unique_by(.artifactName)')
    echo "nativeMatrix=$matrix" >> "$GITHUB_OUTPUT"
    echo '::notice::No changesets. This action does not publish, tag, download artifacts, or create releases; use the durable repository workflow.'
    exit 0
    ;;
  *) echo "::error::callisto status --check exited $status"; exit 1 ;;
esac

# A managed branch must have at most one open PR to its configured base. The
# SHA-suffixed form is an automatic GitHub-token fallback used only when
# updating the long-lived branch would cross a workflow file change.
prs=$(gh pr list --state open --base "$INPUT_BRANCH" --limit 1000 --json number,body,headRefName,headRepository \
  --jq '[.[] | select(.headRepository.nameWithOwner == env.GITHUB_REPOSITORY and (.headRefName == env.INPUT_RELEASE_BRANCH or ((.headRefName | startswith(env.INPUT_RELEASE_BRANCH + "--")) and ((.headRefName | ltrimstr(env.INPUT_RELEASE_BRANCH + "--")) | test("^[0-9a-f]{40}$")))))]')
pr_count=$(jq 'length' <<< "$prs")
case "$pr_count" in
  0) existing='' ;;
  1) existing=$(jq -r '.[0].body // empty' <<< "$prs") ;;
  *) echo "::error::found $pr_count open release PRs for $INPUT_RELEASE_BRANCH -> $INPUT_BRANCH"; exit 1 ;;
esac

if [[ -n "$existing" ]]; then
  body=$(printf '%s' "$existing" | callisto compose-pr-body --existing-body - --label "$INPUT_PR_LABEL" --branch "$INPUT_BRANCH" --format text)
else
  body=$(callisto compose-pr-body --label "$INPUT_PR_LABEL" --branch "$INPUT_BRANCH" --format text)
fi
active_branch=$(jq -r '.[0].headRefName // empty' <<< "$prs")
if [[ -z "$active_branch" ]]; then
  active_branch="$INPUT_RELEASE_BRANCH"
fi

# Rebuild the generated release commit from the current base branch on every
# run. A literal rebase would preserve stale generated version, changelog, and
# changeset-deletion edits rather than recomputing them. GitHub's installation
# token rejects force-updating an old managed ref when its replacement crosses
# an already-reviewed workflow change on the base branch. In that narrow case
# rotate to a deterministic SHA-suffixed managed branch.
release_branch="$active_branch"
if git ls-remote --exit-code --heads origin "refs/heads/$active_branch" > /dev/null 2>&1; then
  git fetch --no-tags origin "refs/heads/$active_branch:refs/remotes/origin/$active_branch"
  if ! git diff --quiet "refs/remotes/origin/$active_branch...HEAD" -- .github/workflows; then
    base_sha=$(git rev-parse HEAD)
    release_branch="${INPUT_RELEASE_BRANCH}--${base_sha}"
    echo "::notice::Rotating the generated release branch because the base branch changed a workflow since the current release PR was created."
  fi
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
git push --force-with-lease origin "$release_branch"

label_exists=$(gh api --paginate "/repos/${GITHUB_REPOSITORY}/labels?per_page=100" --slurp --jq 'any(.[][]; .name == env.INPUT_PR_LABEL)')
if [[ "$label_exists" != true ]]; then
  gh label create "$INPUT_PR_LABEL" --color 0e8a16 --description 'Callisto release packages'
fi
pr=$(jq -r '.[0].number // empty' <<< "$prs")
if [[ -n "$pr" && "$release_branch" == "$active_branch" ]]; then
  gh pr edit "$pr" --title "$INPUT_TITLE" --body "$body" --add-label "$INPUT_PR_LABEL"
else
  new_pr_url=$(gh pr create --head "$release_branch" --base "$INPUT_BRANCH" --title "$INPUT_TITLE" --body "$body" --label "$INPUT_PR_LABEL")
  if [[ -n "$pr" ]]; then
    gh pr close "$pr" --comment "Superseded by $new_pr_url because GitHub requires elevated workflow authority to update this generated branch. The replacement was recomputed from the current base branch and pending changesets."
  fi
fi
