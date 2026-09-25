# Road to v1

Open work only. Verified against the code on 2026-09-25; git history holds the shipped items and the reasoning behind them.

## Decisions for the owner

- Build or drop a workspace-wide check that no `map_err` discards its source error. No gate script or CI wiring exists; `map_err_ignore = "deny"` in `Cargo.toml` is the only partial guard.
- Derive the default tag template from fixed or independent mode, or drop the idea. Today `tag-template` is `None` unless set (`crates/callisto-graph/src/config/resolve.rs`).
- Crate consolidation and the MIT/FSL split.
- Owner items from the PR #101 audit, not rechecked since: the rust-cache pin in orin-dx/actions, the bot-PR check policy, token rotation.

## Correctness

- `Indeterminate` observations drop stderr (`crates/callisto-graph/src/commands/release/provider/registry.rs`, the `ProviderIndeterminateCause::CommandFailed` sites). Add redacted stderr to `ProviderIndeterminateCause`; do not stream it live (`run_quiet` redacts `CARGO_REGISTRY_TOKEN`).
- `fixed_group_target` is SemVer-only and reachable from `plan_snapshot` for an all-PEP 440 fixed group, because `plan_snapshot` skips `pre_mutation_checks`.
- `GroupTable::from_groups` (`crates/callisto-graph/src/config/groups.rs`) skips the conflict check `resolve()` enforces. Test-only today; close it before it gains a real caller.
- The receipt `run_attempt` rationale is wrong: artifact names are already scoped per attempt (run 34702634375 uploaded one name in attempts 1-3). Fix the comment in `.github/workflows/callisto-release.yml` and the message in `.github/tests/verify-release-workflow-policy.sh`.

## Found while rewriting the specs (reproduce before fixing)

Versioning:
- A fixed group with no tagged member bumps from 0.0.0, so members at 1.0.0 with a minor changeset get 0.1.0.
- A fixed group's shared version is SemVer-only: PEP 440 members error, and in pre mode members get a stable version with no `-tag.N`.
- Pre mode never records consumed changesets in `pre.json`, so every pre-mode `version` re-applies them and warns that none are pending.
- `pre.json` is read from `.changeset/` but deleted from the configured `[changesets] dir`; `pre exit` does not stage it.
- `version` after a crash that left changesets on disk bumps again.
- `version --strict` writes and stages everything before failing.
- The same package named in two groups with the same spelling raises `ConflictingGroupNames`, which has no diagnostic code.
- `snapshot` rewrites only dependency specs a patch bump would break, so `core = "1.0.0"` keeps pointing at 1.x after `core` becomes `0.0.0-...`.

Workspace and identity:
- An ecosystem-prefixed `[[package]]` rule (`npm/foo`) applies to an unpromoted Cargo-only `foo`.
- Two id syntaxes: `[[package-set]]` and diagnostics use `cargo:foo`, while changesets, `--package` and output use `cargo/foo`; E103's help suggests `cargo:pkg`.
- E101 (`SplitIdentity`) is defined but never raised.
- Same-name collisions are checked only on each directory's primary id: Cargo `core` + npm `@x/core` in one dir and npm `@x/core` in another load without E100, and the name index is silently overwritten.
- Prefixed `[[package]]` rules never check the package's ecosystems (`pypi:foo` applies to a Cargo-only `foo`); a resolve.rs test locks this in.
- Root detection matches the text `[workspace]`/`[package]` instead of parsing: a commented `# [workspace]` counts, `[workspace.package]` alone does not.
- The `unknown-package` warning lists candidates by primary name, which is wrong for a dual-manifest package whose names differ.
- A qualified id for an unpromoted package matches any ecosystem: `--package cargo/foo` selects an npm-only `foo` instead of failing with E102.
- A manifest that fails to parse is skipped during discovery; when it is a package's only manifest, the package silently disappears.

CLI:
- The generated workflow triggers on the detected default branch but passes no `branch:` to callisto-action, which defaults to `main`.
- `pre exit` with no `pre.json` fails with a raw I/O error.
- `status` piped to a closed reader exits 1 with `callisto::io_error`; `completions` panics.
- `status --check` help text says it signals pending changesets.
- `add --summary x` without `--package` on piped stdin says no flags were given.
- `status` hard-fails on an empty or summary-less changeset instead of reporting a diagnostic; the EmptyChangeset/EmptySummary diagnostics are unreachable.
- `init --forge-repository` is validated only when binaries ship.
- Completions list hidden commands; `schema --type validate` still works.
- `init --dry-run --workflow` does not mention the workflow file; `init` without `origin` leaks git's stderr.
- Error codes mix `callisto::name` and `E###`; only `add` and `pre` JSON reports carry `command`.
- Text output shows internal names (`publish to cratesIo`); `callisto help <command>` is rejected.

Release:
- A `[release]` table requires at least one `[[release.artifact]]` (E197), so any `[release]` forces the CI route.
- `OnDiskVersionDrift` from the Cargo pre-publish check has no diagnostic code.
- The product's GitHub release is published after uploads keyed to the product only; an asset owned by another package does not gate it.
- An upload's prerelease flag comes from the owning package's version, not the product's; a mismatch fails the run with E167.
- The remote-tag check compares only the commit, not the annotation, so a remote tag with another annotation is adopted.
- PyPI observation ignores `Retry-After`; timeouts and spawn failures of observation commands fail with E025 instead of retrying.
- `release plan` without `[release]` ignores a malformed `--orchestration-revision`.
- `release-pr decide` reports duplicate PR numbers and an unsafe snapshot branch as `callisto::error`, not E143/E142.
- `release-pr commit-plan` reads addition bytes from the worktree, not the index.
- Several diagnostics have no help text (E051, E022-E024, E107-E110, `callisto::error`).

## Tests

- Criteria without an asserting test: changeset parse edge cases, cascade `always`/`minor`/dev/E105, apply dual-manifest and `--dry-run` output, E124 re-derivation, release subcommand `--dry-run` rejection, the not-released platform warning. The zero-slots test reads `artifact_slots` instead of `artifactSlots` and always passes.
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
