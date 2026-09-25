# Specification Authority

Current `SPEC-*` artifacts in this directory are Callisto's top-level normative requirements. A `current` specification describes shipped behavior; a `draft` specification is a reviewable contract, not an implementation instruction.

Files in `.claude/semantic-model/` describe verified current implementation. Files in `docs/projects/` describe open work; a plan never overrides a current specification. Only current behaviour and open work are documented: Git history is the record of superseded specs, finished plans and past decisions.

When sources conflict, first reproduce the behavior against the revision named by the current-description material. Correct the specification when the intended contract has changed; otherwise correct the implementation or the current-description material.

## Current

DX v1 (`REQ-DX-V1`, shipped in #126-#150):

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

Release lifecycle:

- `SPEC-SELF-RELEASE-LIFECYCLE` - release lifecycle contract: merge is the only authorization, recovery, provider observation, receipts.
- `SPEC-RELEASE-LIFECYCLE-HARDENING` - the lifecycle as implemented: envelope, transitions, providers, workflow policy, verification tiers.
- `SPEC-RELEASE-DECISION-008` - graph-derived release decision.
- `SPEC-RELEASE-INTENT-MODEL-009` - durable release intent model.
- `SPEC-RELEASE-TRUST-010` - Git source trust and workspace lock.
- `SPEC-RELEASE-CAPABILITY-011` - fresh validation into a private execution capability.
- `SPEC-RELEASE-PR-DECISION-014` - managed release-PR decision and commit plan.
- `SPEC-ARCH-RELEASE-ERROR-TAXONOMY` - typed release errors.

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

- `SPEC-004` (`track-g-matrix-napi-maturin.json`) - `callisto matrix`.
- `SPEC-005` - `ReleaseEntry.is_prerelease`. Its callisto-action release loop criteria were removed in #39.
- `SPEC-TRACK4-RELEASE-PIPELINE-CONTRACT-CORRECTNESS` - status exit codes, tag SHAs, changelog sections. Its callisto-action criteria were removed in #39.

## Draft (not built)

- `SPEC-ARCH-ERROR-SOURCE-PRESERVATION-GATE` - error-source preservation check; the gate script and CI wiring do not exist.
