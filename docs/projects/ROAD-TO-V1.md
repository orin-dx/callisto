# Road to v1

Open work only. Verified against the code on 2026-09-25; git history holds the shipped items and the reasoning behind them.

## Decisions for the owner

- Build or drop a workspace-wide check that no `map_err` discards its source error. No gate script or CI wiring exists; `map_err_ignore = "deny"` in `Cargo.toml` is the only partial guard.
- Derive the default tag template from fixed or independent mode, or drop the idea. Today `tag-template` is `None` unless set (`crates/callisto-graph/src/config/resolve.rs`).
- Crate consolidation and the MIT/FSL split.
- Owner items from the PR #101 audit, not rechecked since: the rust-cache pin in orin-dx/actions, the bot-PR check policy, token rotation.

## Correctness

- `Indeterminate` observations drop stderr (`crates/callisto-graph/src/commands/release/provider/registry.rs`, the `ProviderIndeterminateCause::CommandFailed` sites). Add redacted stderr to `ProviderIndeterminateCause`; do not stream it live (`run_quiet` redacts `CARGO_REGISTRY_TOKEN`).
- `GroupTable::from_groups` (`crates/callisto-graph/src/config/groups.rs`) skips the conflict check `resolve()` enforces. Test-only today; close it before it gains a real caller.
- The receipt `run_attempt` rationale is wrong: artifact names are already scoped per attempt (run 34702634375 uploaded one name in attempts 1-3). Fix the comment in `.github/workflows/callisto-release.yml` and the message in `.github/tests/verify-release-workflow-policy.sh`.

## v1 fix plan

Stacked PRs, merged in this order. Each PR fixes the bugs that break one guarantee and updates its spec criteria in the same change. Most items were reproduced against 0.8.0. Decisions a contributor might reverse (tag identity, directory-keyed identity, native resolution after `version`) also go into ARCHITECTURE.md's design decisions.

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
- Asset uploads are keyed to the owning package, so the product's GitHub release publishes before other packages' assets upload (`commands/release/derive.rs`, `uploads_by_package`). Key them to the product.
- An upload's prerelease flag comes from the owner's version; a mismatch fails every run with E167. Take tag and prerelease from the product's forge release.
- Observation timeouts and spawn failures skip the retry loop (cargo/npm/PyPI adapters, `gh api`, `git ls-remote`) and surface as `callisto::error`; PyPI ignores `Retry-After`. One `run_observation()` and one HTTP transient classifier.
- Remote and local tag checks disagree on the annotation. Compare commit and annotated-ness only.
- `release-pr commit-plan` reads added bytes from the worktree, not the index, and fails from a subdirectory. Verify bytes against the index blob and resolve from the repo root.
- Warn when an artifact's owner is selected without its product.
- Tests: the zero-slots test reads `artifact_slots` (always passes); E124 re-derivation; `--dry-run` rejection on release subcommands; cargo real-registry e2e.

### 2a. Versions are computed correctly
- Fixed groups do their own version arithmetic (`groups.rs::fixed_group_target`): an untagged group bumps from 0.0.0 (1.0.0 + minor gives 0.1.0), pre mode yields a stable version, PEP 440 errors. One bump function shared with `bump_target`; untagged base is the highest member version.
- Pre mode never records consumed changesets, so every run re-applies them. Separate "used this run" from "delete from disk"; skip ids already in `pre.json`.
- `pre.json` location is spelled in four places; with `[changesets] dir` set, exit mode sticks forever. One `pre_json_path()`; `add` honours `[changesets] dir`; `pre exit` stages `pre.json`.
- `version --strict` writes and stages before failing; `--emit-decision` is written before apply. Validate before any write.
- A rerun after a crash bumps again. Refuse per the decision above.
- `pre.json` `initialVersions` keys disagree between aggregate and cascade (`id.name()` vs `display_name()`).
- Tests: cascade `always`, `bump-severity = minor`, dev dependents, E105; `version --dry-run` output.

### 2b. The workspace resolves natively after `version` and `snapshot`
- Invariant: after `version` or `snapshot`, each ecosystem's locked install succeeds (`cargo metadata --locked`, `npm ci`, `pnpm install --frozen-lockfile`, `uv lock --check`).
- Raise the floor of co-released dependents (0.7.2 shipped requiring `callisto-graph = "0.7.0"`).
- Refresh lockfiles by default; add npm, pnpm, yarn, bun and pdm refreshers; a failed refresh is a coded error.
- `snapshot` rewrites only specs a patch bump would break, so `cargo metadata` fails after it. Rewrite every spec that does not cover the snapshot version, in the dependent's ecosystem.
- Test: dual-manifest apply writes both manifests.

### 3. Every package resolves by any of its names
- A qualified selector matches any ecosystem (`--package cargo/foo` selects npm-only `foo`); prefixed `[[package]]` rules ignore ecosystems (a resolve.rs test locks this in). One `IdentityIndex::resolve(selector)`: qualified is an exact (ecosystem, name) lookup, bare searches every name; used by changesets, `add`, `--package`, `[[package]]`, `[[fixed-group]]`, product-package, artifacts, matrix.
- A dual-manifest package cannot be named by its non-primary name.
- Duplicate names are checked only on primary names (two directories can publish `@x/core`). E100 on any (ecosystem, name) claimed twice.
- Three id grammars; `[[package-set]] match = "cargo/foo-*"` matches nothing. Output `eco/name`; accept `eco:name` everywhere; fix E103 help.
- The `unknown-package` warning lists primary names; its advice is impossible to follow.
- An unparseable manifest silently drops its package (or half of a dual-manifest one). Error when admitted; warn otherwise.
- Root detection matches text (`# [workspace]` counts). Parse with toml_edit.
- Delete E101 `SplitIdentity` (divergent names are allowed).
- Group member ambiguity reports E108 instead of E103; E102 help points at a nonexistent config key; `[[package]]` rules that match nothing get no diagnostic.
- Test: the not-released platform warning's two messages.

### 4. Anything `@changesets/cli` accepts, callisto accepts
- An empty changeset (`changeset add --empty`) is rejected with E048. Accept it.
- A private root `package.json` without a version fails with E013. Skip it.
- `pre enter` after `pre exit` is rejected. Re-enter from exit mode.
- A changeset with one unknown entry is consumed, losing the known entries. Do not consume it.
- `status` reports every unparseable changeset as a diagnostic.

### 5. Every failure has a code and help; one output contract
- Architecture test: every error variant has a code and help or is `transparent`. Fixes codeless variants (ParseChangeset, OnDiskVersionDrift, GrammarMismatch, WorkspaceVersionConflict, ConflictingGroupNames and other ConfigError variants), wrappers that hide E020-E025, missing help (E013, E022-E024, E051, E107-E110).
- `E####` registry; split `VcsError::Git` into typed variants; retire `CliError::Other`; keep domain validation out of `Deserialize` (release-pr decide loses E142/E143).
- One JSON envelope with `command` for every report and error.
- `status`, `schema` and `completions` fail or panic on a closed pipe. One output sink; broken pipe exits quietly.
- Git probes echo stderr (`init` without origin, `release --dry-run`). Split probe from exec.
- Completions list hidden commands; `callisto help <cmd>` is rejected; text shows internal names (`cratesIo`, `{:?}`); `--check`/`--strict` help is wrong; `add --summary` without `--package` says no flags were given; wizard prompts go to stdout.
- Delete the legacy `ValidateReport`, `PublishPlan`, `PublishReport` and tag report types and `schema --type validate`.

### 6. What `init` generates works and stays pinned
- The action installs `latest`, ignoring its pinned commit. Install the binary matching the action's commit.
- The workflow passes no `branch:` to callisto-action (defaults to `main`); `default_branch` silently falls back to `main`.
- `--forge-repository` and `--product-package` are dropped without binaries. Validate every init flag up front.
- `init --dry-run` omits the workflow and README from its file list.
- `init` without `origin` per the decision above.
- SECURITY.md hardcodes the current version.

## Design

- Target resolution is Rust-shaped: `triple_host_runner_use_cross` in `crates/callisto-graph/src/matrix.rs` is an 18-triple table. Replace it with facts from `rustc --print cfg`.
- npm platform-manifest discovery is npm-only and directory-scoped (`crates/callisto-graph/src/walk.rs`). Needed for maturin or several platform packages per owner.
- Require `kind` in `[registries]` entries.
- Attest napi `.node` artifacts.

## After 1.0 (additive)

- Effective-config explain surface, built on `ConfigProvenance`. Then drop the default-valued `[cascade]` and `[changesets]` keys from `callisto.toml`.
- `.sha256` sidecars for release assets (also lets the proto plugin verify checksums).
- Glob members in `[[fixed-group]]`.
- Auto-include README, LICENSE and CHANGELOG in release archives.

## Tooling

- `.prototools` floats `moon = "2"`. A moon self-upgrade once required a newer proto than was installed locally; pin the minor.
