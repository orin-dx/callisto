# Road to v1

Unshipped work only. [Owner requirement](../specs/REQ-DX-V1.md), [scoped model and prior art](../architecture/polyglot-v1.md).

Canonical `ecosystem/name` references, historical tag ownership, one tag and release per package, per-distribution publish ordering and the redacted failure tail have shipped. [ADR 12](../adr/0012-canonical-distribution-release-ownership.md) records them.

## Scope and guardrails

- Rust/Cargo, npm/pnpm and Python/PyPI, including napi-rs layouts. Co-located Cargo/Maturin is after v1; separate-directory Cargo and Python packages are supported. Go and NuGet are architecture examples only.
- Native tools and CI own building, packing and authentication. No universal package manager, build scheduler, public plugin platform or cross-registry transaction.
- Committed release authority and provider-observed recovery with no execution journal. Only `execute` receives registry secrets; the repo has no GitHub Environment or reviewer gate.
- Each implementation PR updates the affected specs, tests and docs, with one changeset per independent user-facing topic.

## Open work

| Item | State |
| --- | --- |
| Co-located Cargo/Maturin | After v1. Both a static and a dynamic Python version fail with E007. |
| Production-path registry tests for Cargo and PyPI | The tests drive the publisher argv directly and bypass registry binding and observation. A path through binding is blocked because `registry_endpoint.rs` rejects non-https URLs, loopback included. Decide the approach, then build it or drop it. npm already goes through the real adapter. |
| Native fixture evidence | Cargo and napi clean install and load, mixed workspaces and packed pnpm metadata are not recorded as a verification matrix. |
| First-release onboarding time | The goal of under ten minutes from a fresh repo to a first published release has not been measured. |
| Release target table | `matrix.rs` carries its own 18-triple table instead of reading rustc's target facts. |

## Owner decisions still open

- Private and HTML-only PyPI indexes: [publishing](../publishing.md) says they need explicit support.
- npm channel promotion. Prereleases already publish under their own dist-tag.
- Universal checksums, and wrapper bump severity.
- Inferred-only releases in managed CI: `status` warns, but whether inference may open the release PR is an ADR 2 question.
- Release-commit provenance: ADR 2 documents the unkeyed digest.
- Whether to add a workspace-wide check for discarded source errors. The lints deny `let_underscore_must_use`, `let_underscore_untyped` and `map_err_ignore`.
- npm JSON layout normalization, and the scope of napi `.node` attestation.
- External items: rust-cache pin, bot-PR check policy, token rotation and the retired-crate migration notices.

## After 1.0

OIDC trusted publishing ([plan](OIDC-TRUSTED-PUBLISHING.md)), `cargo-semver-checks` on the promised surfaces (schema output, config and report formats), and a review of the remaining `expect` calls.
