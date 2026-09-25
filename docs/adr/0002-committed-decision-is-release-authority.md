# 2. The committed version decision is the release authority

Status: Accepted

## Context

- After the release PR merges, CI must authorize exactly what reviewers approved before any publish, tag or GitHub release runs.
- A release roster derived from the merged commit's raw diff understands only a direct changeset-to-package match. It rejects every release in which a `[[fixed-group]]` bumps a sibling that no changeset named, which is every multi-package release in a fixed-group workspace.

## Decision

`callisto version --emit-decision <file>` writes the exact version decision it computed (package, target version, inclusion reason) to `.callisto/release-decision.json`, committed in the release PR. After merge, `release plan --from-release-commit --decision <file>` reads that decision and confirms the merged commit's diff matches it exactly, no more and no less. CI never recomputes changeset, group or cascade policy at that boundary. The verified merge is also the authorization: callisto has no approval step of its own, and any extra gate is the adopter's CI choice.

## Options considered

- **Re-derive cascade and group policy in CI after merge** — rejected. "Rather than teach the re-derivation about groups -- a second, incomplete reimplementation of changeset/group/cascade resolution -- this trusts the decision `callisto version` already computed and committed … One decision surface instead of two that can drift apart" (PR #54).
- **Keep the raw-diff roster and special-case fixed groups** — rejected for the same reason: every new policy (linked groups, cascade, prerelease) would need a second implementation.

## Consequences

- What reviewers saw in the release PR is what ships.
- The decision file is a wire contract. Its schema is versioned: `ReleaseDecisionV1::SCHEMA_VERSION` is 2, and the reader accepts 1 and 2 so an older release PR still plans (commit 7027bbfb0).
- A hand-edited decision fails: its deserializer checks entries against a content digest before the diff cross-check runs.
- The release PR must be produced by `callisto version --emit-decision`; a hand-made release commit has no authority.

## Enforcement

- `crates/callisto-cli/tests/durable_release_e2e_tests.rs`: `fixed_group_cascade_bump_without_direct_changeset_is_accepted`, `release_plan_rejects_a_commit_whose_manifest_disagrees_with_its_own_decision`, `release_plan_rejects_a_hand_tampered_decision_file`.
- `derive_release_commit_decision` in `crates/callisto-graph/src/commands/release_decision.rs` and its unit tests (head mismatch, corrupt decision file, claimed package not observed).

## Revisit when

- Release PRs must be produced by something other than `callisto version` (for example a third-party bot) and cannot carry the decision file.
- A needed check can only be made at merge time from data the decision cannot contain.

## Sources

- PR #54 (7199ad70f), body "Why" section
- Code comment on `derive_release_commit_decision`, `crates/callisto-graph/src/commands/release_decision.rs`
- PR #101 (279e32361): "fix(release): emit the committed release decision"
- Commit 7027bbfb0 (reader accepts schema 1 and 2)
