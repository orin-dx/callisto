# 2. The committed version decision is the release authority

Status: Accepted (implemented in #54)

## Context

- After the release PR merges, CI must verify the merged release commit before any publish, tag or GitHub release runs (PR #54).
- Before #54 the release roster was derived from the merged commit's raw diff and understood only a direct changeset-to-package match. It rejected any release in which a `[[fixed-group]]` cascade bumped a sibling that no changeset named (PR #54; code comment on `derive_release_commit_decision`).

## Decision

`callisto version --emit-decision <path>` writes the version decision it computed (package, target version, inclusion reasons) to `<path>`. The managed release-PR action writes it to `.callisto/release-decision.json` by default (`decision_path` input) and commits it in the release PR. After merge, `release plan --from-release-commit --decision <file>` reads that decision and confirms the merged commit's diff matches it exactly, no more and no less. CI never recomputes changeset, group or cascade policy at that boundary. The verified merge is also the authorization: the release workflow has no separate approval gate (no GitHub Environment approval or required reviewers) (incident report §2, requirements 1 and 2; PR #101, "fix(release): remove Environment approval gate").

## Options considered

- **Re-derive cascade and group policy in CI after merge, or keep the raw-diff roster and teach it about groups** — rejected. "Rather than teach the re-derivation about groups -- a second, incomplete reimplementation of changeset/group/cascade resolution -- this trusts the decision `callisto version` already computed and committed … One decision surface instead of two that can drift apart" (PR #54).

## Consequences

- The released roster and versions are exactly those committed in the release PR.
- The decision file is a wire contract. Its schema is versioned: `ReleaseDecisionV1::SCHEMA_VERSION` is 2, and the reader accepts 1 and 2 so an older release PR still plans (commit 7027bbfb0).
- A decision whose entries do not match its own digest fails to decode (E166) before the diff cross-check. The digest is an unkeyed SHA-256, not a signature, so an edit with a recomputed digest that still passes the decoder's roster checks is caught only by the diff cross-check.
- A release commit that does not add or modify the decision file, consumes no changeset, whose manifest versions disagree with the decision, or that leaves a claimed package's changelog untouched is refused (E165). Nothing checks that the decision file was produced by `callisto version --emit-decision`.
- Today `--emit-decision` writes the decision before apply instead of after validation (docs/projects/ROAD-TO-V1.md, v1 fix plan §2a).

## Enforcement

- `crates/callisto-cli/tests/durable_release_e2e_tests.rs`: `fixed_group_cascade_bump_without_direct_changeset_is_accepted`, `release_plan_rejects_a_commit_whose_manifest_disagrees_with_its_own_decision`, `release_plan_rejects_a_hand_tampered_decision_file`.
- `derive_release_commit_decision` in `crates/callisto-graph/src/commands/release_decision.rs` and its unit tests (head mismatch, corrupt decision file, claimed package not observed).

## Revisit when

- Release PRs must be produced by something other than `callisto version` (for example a third-party bot) and cannot carry the decision file.
- A needed check can only be made at merge time from data the decision cannot contain.

## Sources

- PR #54 (7199ad70f), body "Why" section
- Code comment on `derive_release_commit_decision`, `crates/callisto-graph/src/commands/release_decision.rs`
- PR #101 (279e32361), squashed commits "fix(release): remove Environment approval gate" and "fix(release): emit the committed release decision"
- Commit 7027bbfb0 (reader accepts schema 1 and 2)
- `crates/callisto-model/src/release.rs` (`decision_digest`, `ReleaseDecisionV1` deserializer)
- Incident report §2 (`git show 11038b11b^:.claude/plans/RELEASE-SELF-HOSTING-INCIDENT-2026-09-16.md`)
