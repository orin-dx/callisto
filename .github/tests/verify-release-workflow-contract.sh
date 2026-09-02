#!/usr/bin/env bash
# Verify permissions that GitHub only enforces when a guarded release path is
# actually exercised. Keeping this separate from the generic action-pin check
# makes the release authority contract visible and testable on every PR.
set -euo pipefail

workflow=.github/workflows/callisto-release.yml

job_block() {
  local job="$1" next_job="$2"
  sed -n "/^  ${job}:$/,/^  ${next_job}:$/p" "$workflow"
}

require_line() {
  local block="$1" expected="$2" description="$3"
  if ! grep -Fqx "$expected" <<< "$block"; then
    printf 'release workflow contract failed: %s\n' "$description" >&2
    exit 1
  fi
}

version_pr="$(job_block version-pr release-candidate)"
require_line "$version_pr" '      contents: write' 'version-pr must write the managed branch'
require_line "$version_pr" '      pull-requests: write' 'version-pr must create or update the release PR'

release_candidate="$(job_block release-candidate plan)"
require_line "$release_candidate" '          prs=$(gh api --paginate "/repos/${GITHUB_REPOSITORY}/commits/${GITHUB_SHA}/pulls" --jq '\''[.[] | select(.merged_at != null and .base.ref == "main" and (.head.ref == "callisto/version-packages" or (.head.ref | test("^callisto/version-packages--[0-9a-f]{40}$"))))] | length'\'')' 'release-candidate must accept only canonical or SHA-suffixed managed branches'

build="$(job_block build environment-policy)"
require_line "$build" '      contents: read' 'build must read the intent-bound source tree'
require_line "$build" '      attestations: write' 'build must create provenance attestations'
require_line "$build" '      id-token: write' 'build must mint the Sigstore OIDC identity'

ci_workflow=.github/workflows/callisto-ci.yml
checkout_count=$(rg -n 'uses: actions/checkout@' "$ci_workflow" | wc -l)
credential_free_checkout_count=$(rg -n 'persist-credentials: false' "$ci_workflow" | wc -l)
if [[ "$checkout_count" != "$credential_free_checkout_count" ]]; then
  printf 'workflow contract failed: every Callisto CI checkout must disable persisted credentials\n' >&2
  exit 1
fi

require_line "$version_pr" '          persist-credentials: true # zizmor: ignore[artipacked] this isolated version-PR job must authenticate its managed-branch push; no artifacts are uploaded from it.' 'version-pr must document its sole retained Git credential'

if rg -U 'run: \|(?s:.*?)\$\{\{ inputs\.' .github/actions/setup-callisto/action.yml .github/actions/setup-callisto-wasm/action.yml > /dev/null; then
  printf 'workflow contract failed: composite-action inputs must enter shell through named environment variables\n' >&2
  exit 1
fi
