# 3. Reruns observe and adopt landed effects; no persisted execution state

Status: Accepted

## Context

- A release is many external effects (registry publishes, tags, GitHub releases, asset uploads). Any run can die partway.
- Owner requirements: a merged but unpublished version must be recoverable without inventing a newer version; a release is complete only when registries, tags, GitHub release, receipt and source commit agree (incident report §2).
- The incident report proposed persisting execution state after every transition, uploading it on failure and resuming from it (incident §8, SPEC-RELEASE-EXECUTION-TERMINALITY). That was built (#39, #101), then removed in #124.

## Decision

Every run observes every provider before acting and adopts each effect that already landed exactly as planned (a registry version already published is adopted with a warning). Execution state lives in memory for one run. A receipt is written only when every planned operation reached verified success; an incomplete run exits non-zero. Recovering an older merged release is a new workflow dispatch with an explicit `release_source_sha`, not a state file.

## Options considered

- **Persisted execution state file with resume** — rejected. "CI never reads execution state back (every run is a fresh runner), yet an Initial run turned an already-published registry version into E174, so 'Re-run failed jobs' could never finish a partial release" (#124).
- **Recovery commands (`release reconcile`, `release execute --recovery`, `--state`) and Initial/Recovery run kinds** — rejected with the state file in #124 (+830 / -2674 lines). Removed with them: the workspace lock, `AdoptedExact`/`RecoveredExact`, E173, E174 and the release-state schema.
- **Rerun the historical workflow run to recover** — rejected. A rerun of an old GitHub Actions run does not pick up workflow fixes merged later (incident report, requirement 6).
- **Post-publish re-observation pass to build the receipt** — rejected: a flaky registry read could fail a release whose effects had all landed (#122, #124).

## Consequences

- "Re-run failed jobs" and a fresh dispatch both finish a partial release.
- Adoption depends on precise observation. An effect that landed differently (for example a conflicting existing tag) must fail closed, never be adopted.
- No local record of a failed run survives it; the providers are the record.

## Enforcement

- `crates/callisto-cli/tests/release_hardening_tests.rs`: `p01_rerun_after_a_failed_publish_publishes_and_issues_a_receipt`, `p02_rerun_after_a_crate_is_already_published_completes_and_issues_a_receipt`, `p14_rerun_with_a_conflicting_existing_tag_fails_closed`.
- `crates/callisto-cli/tests/release_command_e2e_tests.rs`: `a_failed_run_writes_no_receipt_and_a_rerun_completes`.
- `crates/callisto-cli/tests/durable_release_e2e_tests.rs`: `newer_coordinator_executes_and_reruns_an_older_release_source`; rerun scenarios in `crates/callisto-graph/src/commands/release_simulator.rs`.
- `ReleaseExecutionStateV1` (`crates/callisto-model/src/release.rs`) has no wire shape.

## Revisit when

- Release execution moves to an environment that keeps state between runs, and a provider cannot be observed well enough to adopt its effects.
- A provider offers no reliable way to tell "already landed as planned" from "landed differently".

## Sources

- PR #124 (592396564), commit bodies and PR body; closed PR #122
- PR #101 (279e32361): "reject incomplete quiescent execution", "accept an explicit recovery source"
- PR #39 (eeda4a2f5): durable attested execution
- Incident report §2, §8 (`git show 11038b11b^:.claude/plans/RELEASE-SELF-HOSTING-INCIDENT-2026-09-16.md`)
