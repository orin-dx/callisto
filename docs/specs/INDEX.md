# Specification Authority

Accepted `SPEC-*` artifacts in this directory are Callisto's top-level normative requirements. A specification is accepted only when its `status` is `accepted`; a `proposed` or `draft` specification is a reviewable contract, not an implementation instruction. A `superseded` or `abandoned` specification is history only; `superseded_by` names its replacement.

`docs/01-spec.md` is normative only where it explicitly identifies a requirement. `docs/00-design.md` explains rationale. Files in `.claude/semantic-model/` describe verified current implementation. Files in `.claude/plans/` describe intended work and status. Handoffs are historical context. Neither plans nor handoffs override an accepted specification.

When sources conflict, first reproduce the behavior against the revision named by the current-description material. Correct the specification when the intended contract has changed; otherwise correct the implementation or the current-description material. Do not preserve known contradictions merely to retain a narrative: Git history remains the record of prior decisions.

Legacy `linked_requirement: REQ-*` references have no backing requirement files. New specifications must not add them.

## Design (REQ-DX-V1, not yet built)

Build order: correctness → config schema → release command → CLI surface → status/add → setup core → setup workflow (simple, then matrix).

- `SPEC-DX-CORRECTNESS-PARITY`: release plan gains every `plan-publish` check.
- `SPEC-DX-CORRECTNESS-TRIGGER`: `release-trigger` gates commit inference.
- `SPEC-DX-CORRECTNESS-PROVENANCE-PINS`: regression tests for manifest vs attestation commits.
- `SPEC-DX-CORRECTNESS-CI`: validate stops hiding failures; CI tests the shipped feature set.
- `SPEC-DX-CORRECTNESS-E2E`: publish tests against real local registries.
- `SPEC-DX-CONFIG-RELEASE-SCHEMA`: `[release] forge-repository`; profiles removed.
- `SPEC-DX-RELEASE-COMMAND`: `callisto release` and `--dry-run`; legacy publish commands removed.
- `SPEC-DX-CLI-SURFACE`: 8 visible commands, plain help, colour and tables.
- `SPEC-DX-STATUS-ADD`: status shows planned bumps; `status --check` replaces `validate`; `add` modes.
- `SPEC-DX-SETUP-CORE`: `init` detects facts, asks intent, writes minimal config, previews.
- `SPEC-DX-SETUP-WORKFLOW-SIMPLE`: generated two-job workflow; callisto-action `mode`.
- `SPEC-DX-SETUP-WORKFLOW-MATRIX`: generated plan/build/execute workflow for napi and binaries.

## Current

Release lifecycle:

- `SPEC-SELF-RELEASE-LIFECYCLE` - release lifecycle contract: merge is the only authorization, recovery, provider observation, receipts.
- `SPEC-RELEASE-LIFECYCLE-HARDENING` - the lifecycle as implemented: envelope, transitions, providers, workflow policy, verification tiers.
- `SPEC-RELEASE-DECISION-008` - graph-derived release decision.
- `SPEC-RELEASE-INTENT-MODEL-009` - durable release intent model.
- `SPEC-RELEASE-TRUST-010` - Git source trust and workspace lock.
- `SPEC-RELEASE-CAPABILITY-011` - fresh validation into a private execution capability.
- `SPEC-RELEASE-PR-DECISION-014` - managed release-PR decision and commit plan.
- `SPEC-ARCH-RELEASE-ERROR-TAXONOMY` - typed release errors.
- `SPEC-ARCH-ERROR-SOURCE-PRESERVATION-GATE` - error-source preservation check.

Versioning and graph:

- `SPEC-MANIFEST-PERSIST-001` - explicit `Manifest::persist`.
- `SPEC-APPLY-BATCH-002` - batched manifest writes in `apply_version_plan`.
- `SPEC-001` (`track-b-idempotent-apply.json`) - idempotent apply.
- `SPEC-002` (`track-e-specificity.json`) - package-rule specificity.
- `SPEC-003` (`track-f-resolution-extraction.json`) - package config resolution.
- `SPEC-TRACK1-FIXED-GROUP-CASCADE-CORRECTNESS` - fixed-group cascade.
- `SPEC-TRACK2-PLATFORM-MANIFEST-WRITES` - platform manifest writes.
- `SPEC-TRACK3A-WORKSPACE-MEMBERSHIP` - workspace membership filtering.
- `SPEC-TRACK3B1-IDENTITY-PROMOTION-CORE` - identity promotion.
- `SPEC-TRACK3B2-CONSUMER-COLLISION-SAFETY` - cross-ecosystem name collisions.
- `SPEC-TRACK5-CHANGELOG-PR-BODY-CONSOLIDATION` - changelog and PR body.
- `SPEC-TRACK9-CASCADE-REWRITE-KEY-CORRECTNESS` - cascade rewrite keys.

Matrix and publish preview:

- `SPEC-004` (`track-g-matrix-napi-maturin.json`) and `callisto-matrix.md` - `callisto matrix`.
- `SPEC-005` - `ReleaseEntry.is_prerelease`. Its callisto-action release loop criteria were removed in #39.
- `SPEC-TRACK4-RELEASE-PIPELINE-CONTRACT-CORRECTNESS` - status exit codes, tag SHAs, changelog sections. Its callisto-action criteria were removed in #39.

## Superseded

- `SPEC-RELEASE-EXECUTION-FOUNDATION-007` - by `SPEC-SELF-RELEASE-LIFECYCLE`.
- `SPEC-RELEASE-EXECUTION-012` - by `SPEC-RELEASE-LIFECYCLE-HARDENING`.
- `SPEC-RELEASE-INTERFACES-013` - by `SPEC-RELEASE-LIFECYCLE-HARDENING`.
- `SPEC-RELEASE-PLAN-DURABILITY-001A` - by `SPEC-RELEASE-INTENT-MODEL-009`.
- `SPEC-RELEASE-PLAN-DURABILITY-001B` - by `SPEC-RELEASE-LIFECYCLE-HARDENING`.
- `SPEC-RELEASE-PLAN-DURABILITY-001C` - by `SPEC-RELEASE-LIFECYCLE-HARDENING`.
- `SPEC-RELEASE-SOURCE-PROVENANCE-002` - by 008, 009, 010 and `SPEC-RELEASE-LIFECYCLE-HARDENING`.
- `SPEC-GITHUB-RELEASE-HARDENING-003` - by `SPEC-RELEASE-LIFECYCLE-HARDENING`.

## Abandoned

- `SPEC-SUPPLY-CHAIN-REPRODUCIBILITY-004` - never built.
- `SPEC-DOCUMENTATION-AUTHORITY-005` - only this index shipped.
- `SPEC-RELEASE-LANE-POLICY-006` - never built.
- `SPEC-006` (`SPEC-006-native-artifact-placement.json`) - action placement removed in #39, no replacement.
