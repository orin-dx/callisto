# Road to v1

Open work only. Verified against the code on 2026-09-25; git history holds the shipped items and the reasoning behind them.

## Decisions for the owner

- Build or drop a workspace-wide check that no `map_err` discards its source error. No gate script or CI wiring exists; `map_err_ignore = "deny"` in `Cargo.toml` is the only partial guard.
- Derive the default tag template from fixed or independent mode, or drop the idea. Today `tag-template` is `None` unless set (`crates/callisto-graph/src/config/resolve.rs`).
- Crate consolidation and the MIT/FSL split.
- Release-commit provenance: `release plan --from-release-commit` accepts any commit whose decision file, consumed changeset and manifest diff agree; the decision digest is unkeyed. Decide whether that is enough.
- Owner items pending decision: the rust-cache pin in orin-dx/actions, the bot-PR check policy, token rotation.

## Correctness

- `ApplyPermit` is minted with a literal `false` in `commands/release.rs` (two sites) and cli `init.rs`, so the type does not enforce the dry-run check there.
- npm manifest writes re-serialize the whole file (`callisto-manifests/src/npm.rs`), normalizing hand-formatted layout such as inline arrays.

- `Indeterminate` observations drop stderr (`crates/callisto-graph/src/commands/release/provider/registry.rs`, the `ProviderIndeterminateCause::CommandFailed` sites). Add redacted stderr to `ProviderIndeterminateCause`; do not stream it live (`run_quiet` redacts `CARGO_REGISTRY_TOKEN`).
- `GroupTable::from_groups` (`crates/callisto-graph/src/config/groups.rs`) skips the conflict check `resolve()` enforces. Test-only today; close it before it gains a real caller.
- The receipt `run_attempt` rationale is wrong: artifact names are already scoped per attempt (run 34702634375 uploaded one name in attempts 1-3). Fix the comment in `.github/workflows/callisto-release.yml` and the message in `.github/tests/verify-release-workflow-policy.sh`.

## v1 fix plan

Stacked PRs, merged in this order. Each PR fixes the bugs that break one guarantee and updates its spec criteria in the same change. Most items were reproduced against 0.8.0. PRs 1 and 2b each add an ADR in `docs/adr/`: tag identity and native resolution after `version`. Re-keying packages by directory, with its ADR, is a later PR.

Decisions (owner, 2026-09-25):
- A tag is the same landed effect when it names the same commit and is annotated; the annotation text is not compared, locally or remotely.
- `version` refuses with a coded error when manifests differ from HEAD while changesets are pending (no journal).
- Packages are keyed internally by directory; promotion to `eco/name` is display-only.
- `init` without `origin` writes the config, skips the workflow and forge questions with a warning, and `release` refuses later.
- `status` reports every unparseable changeset as an error diagnostic; `version` still fails on the first.
- One error-code scheme: `E####`, existing numbers kept, `callisto::x` codes mapped into a CLI range, one registry.
- An artifact whose owner is selected but whose product is not emits a warning.
- A dependent released in the same run has its spec on a bumped internal dependency raised to the new version.
- `version` refreshes lockfiles by default; `--no-refresh-lockfiles` opts out.

### 1. A release is complete and consistent
- Cargo real-registry e2e next to the npm (Verdaccio) and PyPI (pypiserver) ones. There is no drop-in local cargo registry; pick one before writing it.

### 2a. Versions are computed correctly
- `resolve()` never inserts explicit provenance for `FIXED_GROUP`, `LINKED_GROUP`, `TAG_TEMPLATE` or `PRE_MAJOR_INFERENCE`, so diagnostics governed by those keys always render "(default)" even when explicitly configured (`crates/callisto-graph/src/config/resolve.rs`).

### 2b. `snapshot` resolves natively for every ecosystem
- `plan_snapshot` (`crates/callisto-graph/src/commands/snapshot.rs`) parses the snapshot tag as SemVer for every package, so a workspace with a Pypi package fails to snapshot: the tag is not a valid PEP 440 version. `cargo metadata --locked`, `npm ci` and `pnpm install --frozen-lockfile` are covered end-to-end (`crates/callisto-cli/tests/native_resolution_e2e_tests.rs`); `uv lock --check` is covered only after `version`, not after `snapshot`.

### 5. Every failure has a code and help; one output contract
- Split the remaining generic `VcsError::Git` parse-failure call sites into typed variants; retire `CliError::Other`; keep domain validation out of `Deserialize` (release-pr decide loses E142/E143).
- One JSON envelope with `command` for every report and error.
- `status`, `schema` and `completions` fail or panic on a closed pipe. One output sink; broken pipe exits quietly.
- Git probes echo stderr (`init` without origin, `release --dry-run`). Split probe from exec.
- Completions list hidden commands; `callisto help <cmd>` is rejected; text shows internal names (`cratesIo`, `{:?}`); `--check`/`--strict` help is wrong; `add --summary` without `--package` says no flags were given; wizard prompts go to stdout.
- Delete the legacy `ValidateReport`, `PublishPlan`, `PublishReport` and tag report types and `schema --type validate`.
- Release derivation has no diagnostics channel: the owner-without-product warning is an `eprintln!` (`commands/release/derive.rs`), so JSON output does not carry it.
- `commands/init.rs::io_err` is pathless, unlike `apply.rs`'s path-carrying `GraphError::ApplyIo` (E122); an init I/O failure doesn't name the file involved.

### 6. What `init` generates works and stays pinned
- The action installs `latest`, ignoring its pinned commit. Install the binary matching the action's commit.
- The workflow passes no `branch:` to callisto-action (defaults to `main`); `default_branch` silently falls back to `main`.
- `--forge-repository` and `--product-package` are dropped without binaries. Validate every init flag up front.
- `init --dry-run` omits the workflow and README from its file list.
- `init` without `origin` per the decision above.
- SECURITY.md hardcodes the current version.

## Design

- Target resolution is Rust-shaped: `triple_host_runner_use_cross` in `crates/callisto-graph/src/matrix.rs` is an 18-triple table. Replace it with facts from `rustc --print cfg`.
- npm platform-manifest discovery is npm-only and directory-scoped (`crates/callisto-graph/src/walk.rs`; [`docs/architecture/identity.md`](../architecture/identity.md)). Needed for maturin platform wheels and for a platform package shared by several owners.
- Require `kind` in `[registries]` entries.
- Attest napi `.node` artifacts.

## After 1.0 (additive)

- Effective-config explain surface, built on `ConfigProvenance`. Then drop the default-valued `[cascade]` and `[changesets]` keys from `callisto.toml`.
- `.sha256` sidecars for release assets (also lets the proto plugin verify checksums).
- Glob members in `[[fixed-group]]`.
- Auto-include README, LICENSE and CHANGELOG in release archives.

## Tooling

- `.prototools` floats `moon = "2"`. A moon self-upgrade once required a newer proto than was installed locally; pin the minor.
