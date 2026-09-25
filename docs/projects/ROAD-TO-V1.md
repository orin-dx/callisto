# Road to v1

Open work only. Verified against the code on 2026-09-25; git history holds the shipped items and the reasoning behind them.

## Decisions for the owner

- Build or abandon `SPEC-ARCH-ERROR-SOURCE-PRESERVATION-GATE`. The gate script and CI wiring do not exist; `map_err_ignore = "deny"` in `Cargo.toml` is the only partial guard.
- Derive the default tag template from fixed or independent mode, or drop the idea. Today `tag-template` is `None` unless set (`crates/callisto-graph/src/config/resolve.rs`).
- Crate consolidation and the MIT/FSL split.
- Owner items from the PR #101 audit, not rechecked since: the rust-cache pin in orin-dx/actions, the bot-PR check policy, token rotation.

## Correctness

- `Indeterminate` observations drop stderr (`crates/callisto-graph/src/commands/release/provider/registry.rs`, the `ProviderIndeterminateCause::CommandFailed` sites). Add redacted stderr to `ProviderIndeterminateCause`; do not stream it live (`run_quiet` redacts `CARGO_REGISTRY_TOKEN`).
- `fixed_group_target` is SemVer-only and reachable from `plan_snapshot` for an all-PEP 440 fixed group, because `plan_snapshot` skips `pre_mutation_checks`.
- `GroupTable::from_groups` (`crates/callisto-graph/src/config/groups.rs`) skips the conflict check `resolve()` enforces. Test-only today; close it before it gains a real caller.
- The receipt `run_attempt` rationale is wrong: artifact names are already scoped per attempt (run 34702634375 uploaded one name in attempts 1-3). Fix the comment in `.github/workflows/callisto-release.yml` and the message in `.github/tests/verify-release-workflow-policy.sh`.

## Tests

- Real-registry e2e for cargo. npm (Verdaccio) and PyPI (pypiserver) exist in `crates/callisto-cli/tests/release_real_registry_e2e_tests.rs`; REQ-DX-V1 asks for all three.

## Design

- Target resolution is Rust-shaped: `triple_host_runner_use_cross` in `crates/callisto-graph/src/matrix.rs` is an 18-triple table. Replace it with facts from `rustc --print cfg`.
- npm platform-manifest discovery is npm-only and directory-scoped (`crates/callisto-graph/src/walk.rs`). Needed for maturin or several platform packages per owner.
- Remove the legacy `PublishPlan`, `PublishReport` and `ValidateReport` types (no command emits them).
- Require `kind` in `[registries]` entries.
- Attest napi `.node` artifacts.

## After 1.0 (additive)

- Effective-config explain surface, built on `ConfigProvenance`. Then drop the default-valued `[cascade]` and `[changesets]` keys from `callisto.toml`.
- `.sha256` sidecars for release assets (also lets the proto plugin verify checksums).
- Glob members in `[[fixed-group]]`.
- Auto-include README, LICENSE and CHANGELOG in release archives.

## Tooling

- `.prototools` floats `moon = "2"`. A moon self-upgrade once required a newer proto than was installed locally; pin the minor.
