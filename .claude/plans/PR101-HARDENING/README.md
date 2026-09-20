# PR 101 hardening track

Status: all audited defects and the agreed structural work are implemented, committed on codex/release-recovery and gated by `just ci`. Hosted CI and a GitHub dry run of the release workflow are still pending. Nothing here is accepted by the owner; the current-description material is docs/07-self-release-lifecycle.md, docs/specs/SPEC-SELF-RELEASE-LIFECYCLE.json, .claude/semantic-model/release-lifecycle.md, and the proposed contract is .claude/specs/SPEC-RELEASE-LIFECYCLE-HARDENING.json.

## Needs the owner

- orin-dx/actions setup-rust pins `Swatinem/rust-cache` by floating tag; it runs inside the jobs holding the OIDC identity and the crates.io token.
- Bot-PR check policy for the managed release PR: the required checks do not start on it without a manual approval or the owner bypass.
- Installer attestation default: `setup-callisto` `verification` input: `require`, `fallback` (default, installs from crates.io), `skip`.
- Token rotation after the incident.
- Acceptance or replacement of every [JC] criterion in SPEC-RELEASE-LIFECYCLE-HARDENING.

## Folder map

- README.md: this file.
- closure-matrix.md: every audit finding with its closure status and evidence.
- defects-and-red-tests.md: defect, fixing commit subject, pinning test.
- w0-ledger.md: falsification ledger; each claim reproduced before work started.
- audit/pr101-arch.md: architecture audit.
- audit/pr101-config-cli.md: configuration and CLI audit.
- audit/pr101-core.md: core release logic audit.
- audit/pr101-perf.md: performance and timeout audit.
- audit/pr101-tests.md: test-suite audit.
- audit/pr101-workflow.md: release workflow audit.

## Deferred

- Data-driven `[release]` catalog for adopters other than Callisto: the target table and coordinator path stay Callisto-specific; no second adopter yet.
- Workspace-hack crate: build-time optimization, not a correctness item.
- npm evidence beyond `npm view`: npm reports no checksum, so its Exact evidence carries none; a stronger source needs a design.
- The Blocked and EffectFailedAndAbsent events are exercised only in tests: no production path can prove absence after a failed effect or block an operation yet.
