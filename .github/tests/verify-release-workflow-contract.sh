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

version_pr="$(job_block version-pr plan)"
require_line "$version_pr" '      contents: write' 'version-pr must write the managed branch'
require_line "$version_pr" '      pull-requests: write' 'version-pr must create or update the release PR'
require_line "$version_pr" "          version_command: 'callisto version --refresh-lockfiles'" 'version-pr must refresh Cargo.lock for coupled Cargo version bumps; the action appends its single authoritative --emit-decision argument'

action=.github/actions/callisto-action/scripts/create-or-update-release-pr.sh
if ! rg -Fqx 'command+=(--emit-decision "$INPUT_DECISION_PATH")' "$action"; then
  printf 'release workflow contract failed: release-PR action must append the exact decision path once\n' >&2
  exit 1
fi

release_candidate="$(job_block release-candidate version-pr)"
require_line "$release_candidate" '          prs=$(gh api --paginate "/repos/${GITHUB_REPOSITORY}/commits/${release_source_sha}/pulls" --jq '\''[.[] | select(.merged_at != null and .base.ref == "main" and .head.repo.full_name == "'\''"${GITHUB_REPOSITORY}"'\''" and (.head.ref == "callisto/version-packages" or (.head.ref | test("^callisto/version-packages--[0-9a-f]{40}$"))))] | length'\'')' 'release-candidate must validate an explicit source only when it is a merged managed branch'
require_line "$release_candidate" '          RELEASE_SOURCE_SHA_INPUT: ${{ inputs.release_source_sha }}' 'release-candidate must pass a dispatch input through an environment variable'
require_line "$release_candidate" '          TRIGGERING_SHA: ${{ github.sha }}' 'release-candidate must pass the triggering commit through an environment variable'
require_line "$release_candidate" '          release_source_sha="${RELEASE_SOURCE_SHA_INPUT:-$TRIGGERING_SHA}"' 'release-candidate must not interpolate user-controlled SHA input into shell'

build_artifact="$(job_block build-artifact build)"
require_line "$build_artifact" '      contents: read' 'each target build must read the intent-bound source tree'
require_line "$build_artifact" '      attestations: write' 'each target build must create provenance attestations'
require_line "$build_artifact" '      id-token: write' 'each target build must mint the Sigstore OIDC identity'
require_line "$build_artifact" '          path: ${{ runner.temp }}/release-intent' 'each target build must keep the release handoff outside either checkout'
require_line "$build_artifact" '          subject-path: ${{ runner.temp }}/release-artifacts/${{ matrix.asset }}' 'each target build must attest its staged artifact bytes'
require_line "$build_artifact" '          bash .github/scripts/build-release-artifact.sh' 'production artifact builds must use the shared preflighted build recipe'
require_line "$build_artifact" '            target: aarch64-apple-darwin' 'the product matrix must build macOS ARM64'
require_line "$build_artifact" '            target: x86_64-unknown-linux-gnu' 'the product matrix must build glibc Linux x86_64'
require_line "$build_artifact" '            target: x86_64-unknown-linux-musl' 'the product matrix must build musl Linux x86_64'
require_line "$build_artifact" '            target: wasm32-wasip1' 'the product matrix must build the WASI plugin'

build="$(job_block build execute)"
require_line "$build" '          callisto release artifact-manifest --intent "${RUNNER_TEMP}/release-intent/release-intent.json" --artifact-dir "${RUNNER_TEMP}/release-artifacts" --out "${RUNNER_TEMP}/release-artifacts/manifest.json"' 'assembly must create the exact four-slot artifact manifest'

plan="$(job_block plan build-artifact)"
require_line "$plan" '          ref: ${{ needs.release-candidate.outputs.orchestration_sha }}' 'planning must use one resolved current orchestration revision'
require_line "$plan" '          path: release-source' 'planning must check out the explicit release source separately'
require_line "$plan" '          ORCHESTRATION_SHA: ${{ needs.release-candidate.outputs.orchestration_sha }}' 'planning must pass the coordinator revision through an environment variable'
require_line "$plan" '            --orchestration-revision "$ORCHESTRATION_SHA" --artifact-repository "$GITHUB_REPOSITORY" \' 'planning must bind the production profile and current coordinator without shell interpolation'
require_line "$plan" '            --from-release-commit "$RELEASE_SOURCE_SHA" \' 'planning must bind the exact release source checkout'

execute="$(sed -n '/^  execute:$/,$p' "$workflow")"
require_line "$execute" '    needs: [plan, build, release-candidate]' 'PR merge is the release approval; execute must depend directly on the plan and verified build, not a GitHub Environment gate'
require_line "$execute" '            cmp "$intent_dir/release-intent.json" "$build_dir/release-intent/release-intent.json"' 'execute must reject a build handoff whose immutable intent differs from planning'
require_line "$execute" '          path: ${{ runner.temp }}/release-intent' 'execute must download release-intent outside the workspace -- callisto release execute re-checks release trust before every dispatch, which fails closed on any untracked file in the worktree'
require_line "$execute" '          path: ${{ runner.temp }}/release-build' 'execute must download release-build outside the workspace -- callisto release execute re-checks release trust before every dispatch, which fails closed on any untracked file in the worktree'
require_line "$execute" '          path: release-source' 'execute must use an explicit release-source checkout'
require_line "$execute" '          ORCHESTRATION_SHA: ${{ needs.release-candidate.outputs.orchestration_sha }}' 'execution must pass the coordinator revision through an environment variable'
require_line "$execute" '          args=(--source-root "$GITHUB_WORKSPACE/release-source" --intent "$intent_dir/release-intent.json" --receipt "$receipt_dir/release-receipt.json" --orchestration-revision "$ORCHESTRATION_SHA" --profile production)' 'execute must pass the explicit source checkout, coordinator revision, and mandatory receipt path to the current Callisto coordinator'

ci_workflow=.github/workflows/callisto-ci.yml
checkout_count=$(rg -n 'uses: actions/checkout@' "$ci_workflow" | wc -l)
credential_free_checkout_count=$(rg -n 'persist-credentials: false' "$ci_workflow" | wc -l)
if [[ "$checkout_count" != "$credential_free_checkout_count" ]]; then
  printf 'workflow contract failed: every Callisto CI checkout must disable persisted credentials\n' >&2
  exit 1
fi

require_line "$version_pr" '          persist-credentials: false' 'version-pr never git-pushes -- it commits through the forge API -- so it must not persist Git credentials'

if ! rg -Fqx '      - run: just release-workflow-behavior' "$ci_workflow"; then
  printf 'workflow contract failed: PR CI must run the behavioral release workflow harness\n' >&2
  exit 1
fi

if ! rg -Fqx '  release-artifact-preflight:' "$ci_workflow"; then
  printf 'workflow contract failed: PR CI must preflight every production artifact build recipe\n' >&2
  exit 1
fi

if ! rg -Fqx '        run: just workflow-contracts' "$ci_workflow" \
  || ! rg -Fqx '    bash .github/tests/test-release-artifact-build-script.sh' justfile; then
  printf 'workflow contract failed: CI must test the shared release artifact build script\n' >&2
  exit 1
fi

if ! rg -Fqx '          - id: macos-arm64' "$ci_workflow" \
  || ! rg -Fqx '            target: aarch64-apple-darwin' "$ci_workflow" \
  || ! rg -Fqx '            target: x86_64-unknown-linux-gnu' "$ci_workflow" \
  || ! rg -Fqx '            target: x86_64-unknown-linux-musl' "$ci_workflow" \
  || ! rg -Fqx '            target: wasm32-wasip1' "$ci_workflow"; then
  printf 'workflow contract failed: artifact preflight must cover all four product targets\n' >&2
  exit 1
fi

execute="$(sed -n '/^  execute:$/,$p' "$workflow")"
require_line "$execute" '          persist-credentials: true' 'execute pushes the release tag with a plain git push (dispatch_tag) -- unlike every other job it has no other way to authenticate, so it must persist Git credentials'

if rg -n '^  environment-policy:$|^    environment: release$' "$workflow" > /dev/null; then
  printf 'release workflow contract failed: a merged release PR is the only approval; no Environment reviewer gate may delay execute\n' >&2
  exit 1
fi

if rg -U 'run: \|(?s:.*?)\$\{\{ inputs\.' .github/actions/setup-callisto/action.yml .github/actions/setup-callisto-wasm/action.yml > /dev/null; then
  printf 'workflow contract failed: composite-action inputs must enter shell through named environment variables\n' >&2
  exit 1
fi
