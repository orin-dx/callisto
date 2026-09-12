# callisto-graph

## 0.7.1

- **Confirm a Cargo publish landed on the registry before marking it done**
  
  `dispatch_registry` only confirmed npm publishes against the registry after a successful publish call; Cargo went straight from "cargo publish exited 0" to `Published`, with no check that crates.io's index had actually caught up. If a dependent crate's publish ran before that propagation finished, its own `cargo publish --locked` verification build could fail to resolve the dependency, and the failure wouldn't be recognized as recoverable propagation lag.
  
  Cargo publishes are now confirmed the same way npm's are: after a successful `cargo publish`, `cargo info <pkg>@<version>` must also confirm the registry shows it before the operation is marked `Published`. If it doesn't yet, the operation stays `Attempting` and reports `RegistryPublishUnconfirmed` -- re-running reconciliation once the registry catches up resolves it, with no need to regenerate the release intent.
  
  PyPI's publish path has the same gap and is deliberately left open: this workspace has no PyPI package or credentials to verify a fix against.
- **Retry the registry-publish confirmation check with backoff instead of failing on the first miss**
  
  The registry-publish confirmation added for Cargo and already used for npm checked the registry exactly once. Propagation lag is normally sub-second, but a single check right after a publish could still land inside that window and fail immediately with `RegistryPublishUnconfirmed`, even though a moment later it would have succeeded on its own.
  
  The confirmation check now retries up to 3 times (4 checks total) with exponential backoff (2s, 4s, 8s -- 14s worst case) before reporting unconfirmed. Only registry propagation lag is retried this way; a hard error from the check itself is never retried.
  
  Lives in `callisto release execute` itself, not the release action's shell script -- the action has no registry-specific logic of its own to retry.

## 0.7.0

- **Fix three error/diagnostic messages that told the operator the wrong cause**
  
  - `callisto-vcs`: `ReleaseWorkspaceLock::acquire` reported every lock-acquisition failure -- permission denied, disk full, a filesystem that doesn't support `flock`, or any other I/O error -- as "another Callisto release already holds this workspace lock". It now distinguishes true contention (matched against `fs2::lock_contended_error()`'s `ErrorKind`, the same signal `fs2` itself uses) from other I/O failures, and reports the real underlying cause for the latter instead of misattributing it to a held lock.
  - `callisto-graph`: `IdentityResolver::resolve` collapsed both an unsupported-ecosystem case and a genuine `PackageId` parse failure into `GraphError::AmbiguousName`, which is semantically wrong for both (neither is a name-ambiguity problem) and discarded the real reason. Two new variants -- `UnsupportedIdentityEcosystem` and `PackageIdentifierParse` (carrying the underlying `PackageIdParseError`) -- now report each failure's own accurate cause.
  - `callisto-cli`: `callisto add --package name:severity` rejected an invalid severity with "Must be patch, minor, or major.", omitting `none`, which `Severity::from_str` has always accepted. The message (and the `--package` flag's help text) now lists all four accepted values.
- `solve_cascade`'s Fixed-group convergence block now skips a group member absent from the workspace instead of writing it into the cascade outcome unconditionally. If a `[[fixed-group]]` in `callisto.toml` still named a package removed from the workspace, and a live sibling in that group received nonzero severity during cascade convergence, the stale package id was inserted into `CascadeOutcome.severities`/`.targets` with no `input.base` guard -- the Linked-group block right above it already had this guard (from a prior fix), but the Fixed-group block was never updated to match. The stale id then reached `plan_version`'s `pkg_map.get(id).copied().unwrap()` (built only from live graph packages) and panicked, crashing `callisto version` instead of degrading gracefully the way `aggregate::union_fixed` already does for the identical scenario. The Fixed-group block now filters into live members, skips stale ones, and emits an `UnknownPackage` diagnostic (governed by `ConfigKey::FIXED_GROUP`) for each one instead.
- **Delete unused abstractions with zero real callers**
  
  Removed as pure dead-code deletions -- each had exactly one implementation (or none) reachable only through its own tests, with every real call site already going through the underlying concrete type or free function directly:
  
  - `PackageIdentityResolver` trait (`callisto-model`) -- a one-line delegate to `PackageId::matches`.
  - `ChangesetStorage` trait (`callisto-model`, re-exported from `callisto-manifests`) -- a one-line method-call spelling of the free function `atomic_write`.
  - `ManifestCstEditor` trait and its blanket impl (`callisto-manifests`) -- combined mutate-and-persist in one call, which no real caller used and which would have been the wrong shape anyway: `apply.rs` deliberately calls `write_version`/`update_dependency_spec` and `persist` as separate steps to batch several writes before one flush.
  - `ReleaseEffectAdapter` trait and `PreparedReleaseEffectAdapter` (`callisto-graph`) -- `execute_release`/`execute_release_with_artifacts` now call the prepared capability's dispatch directly instead of going through a single-implementation generic seam.
  - `RawMoonYml`/`RawMoonExtensions`/`RawMoonCallistoConfig` (`callisto-graph`) -- a config schema nothing in this crate ever deserialized a `moon.yml` file into.
  - `ReportPresenter` trait (`callisto-cli`) -- only implemented by a test-only fake built solely to exercise the trait's own default method.
  - `--skip-publish-precheck` CLI flag and `check_credentials` (`callisto-cli`) -- the flag was parsed and immediately discarded; `check_credentials` is `#[cfg(test)]`-gated and was never compiled into the production binary.
  
  No behavior change for any real caller.
- Four instances of a value being re-derived instead of reused, all fixed the same way (compute once, have both branches consume the same value):
  
  - `crates/callisto-cli/src/commands/status.rs`'s `--check` exit code recomputed `has_changesets` from `report.packages` instead of reading the field `callisto-graph`'s status command already computed and stored on `StatusReport`.
  - `crates/callisto-graph/src/commands/validate.rs`'s `--staged` and `--since` branches each ran their own `git diff`, error-mapped it, and filtered changesets identically, differing only in the git args. Factored into one `changesets_touched_by` helper.
  - `crates/callisto-graph/src/commands/tag.rs`'s floating-major tag computation (template lookup, version extract, grammar resolve, parse, render, existence check) was written out fully in both the dry-run preview branch and the write branch. Factored into `plan_floating_major`, called by both; the permit still gates only the actual `create_floating_major` call.
  - `crates/callisto-cli/src/commands/publish.rs` and `plan_publish.rs` independently wired up the identical `load_workspace` -> `plan_publish` pipeline. Factored into a shared `plan_publish::build_plan` both commands call.
- **Fix `fixed_group_target` reading a stale group member's (missing) slot in `base`, corrupting the shared alignment version**
  
  Track 1 (Fixed-group convergence) in `solve_cascade` passed the raw `GroupDef` into `groups::fixed_group_target`, which iterated every declared member -- not just the ones still present in `input.base` -- when picking the group's alignment base. A group member removed from the workspace but still declared in `callisto.toml`, if it happened to carry a release tag from before its removal, could be picked as `released[0]`; `base.get(&released[0])` then missed (the stale id has no entry in `base`) and silently fell back to the hardcoded `Version::semver(1, 0, 0)` default, corrupting the group's shared target version for every live sibling. `fixed_group_target` now takes an already-filtered `live_members` slice instead of the raw `GroupDef`, mirroring the stale-member guard the Linked-group block directly above it already applies -- a stale member can no longer occupy the alignment slot regardless of declaration order or tag history.
  
  **Fix `ReleaseOperationId`/`ArtifactSlotId`'s `Ord` comparing a `Version` field via `render()` instead of `(grammar, raw)`**
  
  Residual instance of the same defect class as the earlier `ReleaseOperationRole` fix (which stopped `Ord` from dropping `attestation_policy`): both hand-written `Ord` impls keyed their `version: Version` field on `version.render()` alone, dropping `version.grammar()`. The identical literal string parses under more than one grammar (`"1.2.3"` is valid both as SemVer and PEP 440), so two otherwise-identical `ReleaseOperationId`/`ArtifactSlotId` values that are `Eq`-distinct only by grammar compared `Ordering::Equal`, violating the invariant `BTreeSet`/`BTreeMap` require: `a.cmp(b) == Equal` iff `a == b`. Both now key the version field on `(grammar, raw)` via a shared `version_ord_key` helper.
- Fixed/linked group severity computation ("max severity across a group, propagate to every member") was independently re-derived in four places: `aggregate::union_fixed`/`union_linked`, two inline blocks in `cascade::solve_cascade`, and `groups::fixed_group_target` (which redundantly recomputed a value its caller had already computed). Only `aggregate`'s copies guarded against a group member removed from the workspace; `cascade`'s Linked-group block had no such guard, so a stale member reachable through a live sibling's severity bump during cascade convergence could hit a `MissingField` crash.
  
  Added `GroupDef::package_members()` and `GroupDef::max_severity()` as the single definitions of these two operations, used by all four call sites. `cascade::solve_cascade`'s Linked-group block now applies the same stale-member guard `aggregate` already had. `fixed_group_target` now takes the caller's already-computed severity instead of recomputing it. Also deleted `CascadeSolver`, a trait with zero implementations and zero call sites (its own doc comment noted no implementation exists; the free function is called directly everywhere).
- `IdentityResolver::resolve` did a fresh `fs::read_to_string` + full manifest parse on every call with no memoization. `MoonProjectLocator::declared_edges` re-resolved every project's identity a second time (`projects()`, called earlier in the same flow, already resolved it once) and, worse, re-resolved a dependency edge's target once per edge rather than once per unique target project -- a widely-depended-on package (e.g. a shared crate with 30 internal dependents) had its manifest read and parsed 30+ times in a single `Workspace::load`.
  
  `IdentityResolver` now memoizes `resolve`'s result by `(path, ecosystem)` for its whole lifetime. Since `MoonProjectLocator` holds one `IdentityResolver` across both `projects()` and `declared_edges()`, a given manifest is now read and parsed at most once per run, with no change needed to `declared_edges` itself.
- `ManifestWalkResolver::build`'s napi-platform-package detection did its own raw `fs::read` + `serde_json::from_slice` on every discovered npm `package.json`, entirely outside `callisto-graph`'s `manifest_cache` -- the same mechanism that exists precisely so a manifest path is read and parsed at most once per run. The exact same file was then opened again moments later, through the cache, for `publish_targets`/`iter_dependencies`.
  
  `Manifest` gains an `npm_role() -> Option<NpmRole>` method (default `None`), overridden by `PackageJson` to derive the napi platform/arch/abi role from the document already parsed at `open()` time. `ManifestWalkResolver::build` now calls `.npm_role()` on the manifest handle it gets from `open_cached`, so each npm manifest is read from disk once, not twice.
- **Add `OpenContext::for_workspace_root`; stop reimplementing its construction independently in four places, and stop re-reading a platform owner's manifest once per platform target**
  
  `callisto-manifests` gains `OpenContext::for_workspace_root(root)`, the single reusable constructor for the "check `Cargo.toml` exists, load `WorkspaceCargoResolver` inheritance, detect npm workspace kind" sequence. `callisto-graph`'s `ManifestWalkResolver::build`, `Workspace::base_versions`, `apply_version_plan`, and `plan_version`'s manifest-write path each independently reimplemented this same ~15-line block; all four now call the shared constructor. No behavior change -- the resolution logic is byte-for-byte the same, just no longer copy-pasted.
  
  `plan_version`'s platform-target loop also stopped re-opening (re-reading and re-parsing from disk) a fixed-group owner's canonical manifest once per platform-manifest sibling purely to check for an existing matching optional dependency -- napi/native cross-compile owners typically have 4-8+ platform targets. That read now routes through the workspace's existing read-only `manifest_cache`, so the owner's manifest is opened at most once per run regardless of how many platform siblings it has.
- **Parameterize six "one arm per kind" implementations on the value that varied between arms, instead of writing each arm out separately**
  
  No behavior change for any real caller in this set -- each factors an existing, independently-tested code path into a single shared implementation:
  
  - `callisto-manifests` gains `Requirement` (with `parse`/`render`), a real PEP 508 dependency-specifier type backed by a parse-render-parse idempotence property test. Replaces three independent from-scratch decompositions of the same marker-split/operator-split/extras-split logic in `python.rs` (`iter_dependencies`, `update_dependency_spec`, `update_optional_dependencies`). Rewriting a dependency's extras list or marker now goes through `Requirement::render`'s normalized form (e.g. `[a, b]` -> `[a,b]`, `; marker` -> `;marker`) rather than re-splicing the original raw substrings -- no existing test encoded the old whitespace-preserving behavior.
  - `callisto-manifests`'s `cargo.rs` factors `set_scalar_preserving_decor` and `set_dependency_version` out of the member-manifest (`CargoToml`) and workspace-root (`WorkspaceCargoResolver`) write paths, which independently hand-rolled the same decor-preservation dance for `[package].version` and for a dependency's bare-string/inline-table/full-table value shapes.
  - `callisto-graph`'s `config::resolve::load` factors `parse_package_config_fields` out of its `[[package]]` and `[[package-set]]` loops, which ran the identical release-trigger/tag-template/changelog/pre-major-inference/publish-to parsing sequence into an identical `PackageConfig` literal, differing only in the pattern-parser type and an error-message prefix.
  - `callisto-graph`'s `config::groups::GroupTable::validate_syntactic` and `::resolve` now loop over `[GroupKind::Fixed, GroupKind::Linked]` internally instead of writing the fixed arm and linked arm out separately. `validate_syntactic` previously had no direct unit tests; this change adds eight, covering the asymmetric cross-kind member-conflict check the single shared implementation had to reproduce exactly.
  - `callisto-changelog`'s `write::prepend` now computes `rest` once across its three header-shape branches (blank line after the H1, no blank line, no matching H1 at all) and runs the shared five-statement splice a single time, instead of each branch repeating the same splice after computing its own `rest`.
  - `callisto-moon`'s `locator.rs` factors a single `detect_single_ecosystem` helper for `declared_edges`'s from/to ecosystem lookup (previously two copies of the same if/else-if chain), and routes both `projects()`'s multi-ecosystem enumeration and `declared_edges`'s single-ecosystem lookup through `Ecosystem::CANONICAL` instead of separately hardcoded manifest-filename checks.
- **Fix: Poetry-style version specs and negated workspace globs were silently dropped from the dependency graph**
  
  `python.rs`'s `iter_dependencies` tried to parse every `[tool.poetry.dependencies]` version spec directly as PEP 440 via `VersionReq::parse`. Poetry's own caret (`^1.2`), tilde (`~1.2.3`), and wildcard (`1.*`) syntax is not valid PEP 440, so that parse always failed for those specs and the dependency was silently omitted from `iter_dependencies`'s output entirely -- no warning, no error, no entry. Since the version-bump cascade walks exactly that iterator to find who depends on what, any package depending on an upstream package via caret/tilde/wildcard syntax was invisible to the cascade and never got bumped when its dependency did.
  
  Added `normalize_poetry_version_spec` (plus `normalize_caret`/`normalize_tilde`/`normalize_wildcard` helpers) to `python.rs`, which expands Poetry's caret/tilde/wildcard clauses into equivalent PEP 440 ranges before the `VersionReq::parse` attempt, while leaving already-PEP-440-compatible clauses (`>=1.2,<1.5`, `~=1.2.3`, `==1.2.*`) untouched. A spec that still can't be represented after normalization -- or a Poetry dependency with no `version` key at all (a path/git/url dependency) -- now falls back to `DepSpec::Opaque` instead of vanishing, so the dependency edge always survives into the graph. Also fixed two sibling gaps discovered while auditing this code path: a Poetry dependency written as a dotted table (`[tool.poetry.dependencies.foo]`) was previously unrecognized by the `toml_edit::Item` match at all (a different silent-drop mechanism, now unified onto `as_table_like()`), and the parallel PEP 621 `[project].dependencies` block had the same silently-skip-on-parse-failure pattern, now also falling back to `DepSpec::Opaque` instead of dropping.
  
  Separately, `callisto-graph`'s `build_globset` (used by Cargo/uv `members`/`exclude` and npm/pnpm/Yarn `workspaces`/`packages`) compiled every pattern -- including ones with a leading `!` -- as a plain positive-match glob, since `globset::GlobSetBuilder` has no negation concept of its own. A workspace config like `["packages/*", "!packages/excluded-one"]` therefore compiled `!packages/excluded-one` into a glob matching paths that literally start with `!`, which never matches anything real, so the "excluded" package silently remained a workspace member. `build_globset` now returns a `NegatableGlobSet` that splits entries into positive and negative (`!`-stripped) `GlobSet`s and admits a path only when it matches a positive pattern and no negative one, mirroring how npm/pnpm/Yarn actually resolve workspace globs. `crates/callisto-graph/src/config/pattern.rs` and `crates/callisto-graph/src/tags.rs` also use `globset`, but each compiles exactly one glob per rule with no list-of-patterns-with-negation shape, so neither has the same gap.
- `render_pr_body_from_plan` looked up each bump's matching `ChangelogWrite` with `plan.changelog_writes.iter().find(...)` inside the loop over `plan.bumps` -- an O(bumps * changelog_writes) scan for every PR body render. Builds a `HashMap<&PackageId, &ChangelogWrite>` once before the loop instead, mirroring the `pkg_map` precedent already used by `plan_version` in `version.rs` for the identical anti-pattern.
- **Restructure the release PR body to group by change instead of by package, and drop redundant/misleading sections**
  
  A single `.changeset/*.md` file can name several packages, and its summary was copied verbatim into every one of them (correct and desirable for each package's own standalone `CHANGELOG.md`) -- but the PR body then rendered that same text once per package it touched, producing bodies with the same paragraphs repeated multiple times for any release spanning a fixed/linked group or a multi-package changeset. A real 10-package release PR measured at 629 lines.
  
  - `compose-pr-body`'s `### 📦 What's Changing` section (renamed from `Package Release Details`) now groups by the underlying change (a changeset file, a commit, a dependency/peer bump) and lists every package it affects in one block, instead of one full block per package. A package bumped only via a fixed/linked group with no direct change of its own is now named in a short note instead of getting an empty-looking collapsible.
  - The whole section is wrapped in one collapsible, closed by default and open only when the release contains a major bump; each change's own block follows the same rule. A routine minor/patch release now renders as just the summary table, one note, and a single closed toggle.
  - The redundant "#### Version Change" sub-heading is gone (already shown one line above in each block's `<summary>`), and the major-bump callout uses `[!WARNING]` instead of `[!IMPORTANT]`.
  - The summary table's Bump column now includes a severity emoji (🔴/🟡/🟢) for at-a-glance scanning.
  - The "Suggested PR Labels" line is removed: the label it named was never merely suggested (the release-PR script always applies it via `gh pr create --label`/`gh pr edit --add-label`), so the line was both redundant with GitHub's own label UI and factually misleading. `compose-pr-body`'s now-pointless `--label` flag is removed along with it (`ComposePrBodyArgs`/`PrBodyOptions` drop the `labels` field); the release-PR action script no longer passes `--label` to `compose-pr-body`, only to the real `gh pr create`/`gh pr edit` calls where it's actually applied.
  
  No changes to changeset authoring, parsing, or per-package `CHANGELOG.md` output -- this is scoped entirely to the PR body, the one place the same fanned-out content is read together in a single sitting.
- **Order the release PR's summary table and change list by severity, major first**
  
  A reviewer merging a release PR is really answering one question: is anything breaking? Before this change, both the summary table and the "What's Changing" list rendered in plan order (roughly topological/declaration order), so a major bump could land anywhere among a dozen rows — a reviewer had to scan the whole table to be sure nothing was breaking, even though a major bump's own detail block already auto-opens.
  
  Both the table and the change list are now sorted by severity descending (major, then minor, then patch/none), stable within each tier so otherwise-equal rows keep their original relative order. A breaking change is now always the first thing visible, in both places, regardless of where its package name or declaration would otherwise have put it.
- **Stop hiding the release PR's change list behind an extra collapsible**
  
  The outer `<details>` wrapper added around `### 📦 What's Changing` (closed by default unless a major bump is present) made the entire list of what changed invisible until clicked — including for a routine minor-only release, where a reviewer had to open the wrapper just to see the *names* of the changes before deciding whether to read any of them.
  
  The outer wrapper is removed; the list of changes (each one's own `<summary>` naming the change and the packages it affects) is now always directly visible, replaced only by a plain `**N change(s) across M package(s):**` count line. Each individual change's own body still collapses by default and opens automatically when it contains a major bump — only the actual prose text collapses, never the fact that a change exists.
- **Fix the durable release executor's registry-publish and tag-creation argv; delete the disused `PublishOrchestrator`/`SubprocessRegistryClient` path**
  
  The durable release executor's `dispatch_registry` (the live `callisto release execute` publish path) was rewritten independently of the older `PublishOrchestrator`/`SubprocessRegistryClient` pipeline and silently dropped real behavior along the way:
  
  - **npm**: it hardcoded plain `npm publish`, regardless of the workspace's actual package manager. A pnpm or yarn workspace published with the wrong tool -- a live, real bug. It now detects `pnpm-lock.yaml`/`yarn.lock`/`bun.lock(b)` and builds the correct `pnpm publish --filter`/`yarn workspace ... npm publish`/`bun publish`/`npm --workspace` invocation.
  - **cargo**: `cargo publish` ran with no `--locked` and no check that the on-disk manifest version matched the release plan. Both are restored; a mismatch now fails fast with `GraphError::OnDiskVersionDrift` instead of silently publishing a stale version.
  - **pypi**: it built into a throwaway temp directory and uploaded whatever landed there. It now builds into `dist/` and uploads the exact `dist/<normalized-name>-<version>*` glob, matching the older client's intentional scoping.
  - **git tag creation**: it inlined its own `git tag --no-sign -a ...` call with no `--` end-of-options guard against a malicious/malformed tag name. It now goes through `callisto_vcs::GitDataSource::create_tag`, which already has that guard -- extended with a new `TagSignPolicy` parameter (`RespectRepoConfig` for the existing non-durable `callisto tag` path, `ForceUnsigned` for the durable executor) so both safety properties hold together instead of one replacing the other.
  
  The corrected argv construction and output classification for all three registry ecosystems now live in a new pure module, `callisto_graph::commands::registry_argv` (no `CommandRunner`/orchestration logic of its own). `PublishOrchestrator`, `SubprocessRegistryClient`, `AlwaysRetryPolicy`, `SystemTimeProvider`, and `parse_retry_after` are deleted: nothing in production ever constructed them (`callisto publish`'s CLI handler is an explicit dry-run-only compatibility preview; `release execute` is the only real effect path), and their retry/backoff/pre-check policy has no equivalent in the durable executor's model, which persists `Attempting` state and relies on a later re-invocation to retry rather than an in-process retry loop.
- What each `PublishTarget` variant means -- whether it's dispatchable, how it contributes to a package's semantic fingerprint, and whether it carries an explicit registry override -- was independently re-encoded in three separate match statements (`callisto-graph`'s `target_fingerprint`, `prepared_registry_binding`, and `publish.rs`'s per-target dispatch loop), kept in sync only by convention with `callisto-model`'s existing `PublishTarget`/`Ecosystem` capability layer.
  
  Extended that existing layer instead of adding a second one: `PublishTarget` gains `registry_override()` (the `Npm.registry`/`Pypi.index`/`NuGet.source` payload field, unified) and `is_implemented()`. `PublishTarget` and `Ecosystem` are not 1:1 -- `GitHubRelease` is a VCS release action with no backing package `Ecosystem`, and `None` is the "not configured to publish" sentinel -- so `is_implemented()` special-cases both (`GitHubRelease` is always unimplemented, `None` is vacuously implemented) and otherwise defers to `Ecosystem::is_implemented()`. `target_fingerprint` now reuses the existing `config_str()` instead of re-hardcoding its own kind strings; `prepared_registry_binding` now calls `registry_override()` instead of its own separate match; `publish.rs`'s dispatch loop now decides its `PublishTargetNotImplemented` diagnostic via `is_implemented()` instead of independently naming `NuGet`/`GitHubRelease`. Pure refactor -- no behavior change for any existing test case.
- **Delete unused abstractions with zero real callers; share tag-glob compilation**
  
  - `GitVcsProvider` trait and its `GitRepository` impl (`callisto-vcs`) -- fully superseded by the already-polymorphic `GitDataSource` trait (`GitRepository`/`ShellGit`/`GitAccess`, used generically throughout `callisto-graph`); `GitVcsProvider` had exactly one impl and zero callers anywhere in the workspace.
  - `last_tag_for` and its `select_from_tags` helper (`callisto-graph`) -- re-exported from the crate root "for API compatibility" but the only callers were `last_tag_for`'s own unit tests; `TagIndex::build` already resolves tags via `select_from_tags_cached`.
  - Added `callisto_vcs::compile_tag_glob` -- the one real, shared implementation of "compile a tag glob and map a compile failure to `VcsError::InvalidGlob`", now used by both `GitDataSource` backends and by `callisto-graph`'s tag matching instead of three independent copies.
  
  No behavior change for any real caller.
- **Report registry-publish propagation lag with its own error, wire up missing `[lints]` tables**
  
  `dispatch_registry`'s npm publish-succeeded-but-registry-not-yet-consistent branch used to return `GraphError::ReleaseIntentStale`, whose help text ("no release operation was authorized") is false at that call site -- an operation *was* authorized and did run; the real cause is registry propagation lag. It now returns a new `GraphError::RegistryPublishUnconfirmed { package, version }` (E157) with accurate help text. The check itself (`Ecosystem::Npm` + `PublishOutcome::Published` + registry not yet showing the version) is unchanged; only the error reported when it fires changed. Extracted the check into `require_registry_confirmation` for direct unit coverage.
  
  `callisto-fixtures` and `callisto-moon` were missing the `[lints]` table every other crate in the workspace has, so `unsafe_code = "forbid"` and the shared clippy lint set were silently never enforced on them. `callisto-fixtures` had zero latent violations. `callisto-moon`'s `tests/moon_wasm_sandbox.rs` legitimately mutates process-wide env vars (`WARPGATE_PLUGINS_DIR`, `PATH`) under `Once`/mutex-serialized, individually SAFETY-commented `unsafe` blocks for test isolation -- `forbid` cannot be locally overridden by any in-source attribute at any nesting depth, so `callisto-moon`'s `[lints.rust]` duplicates the workspace's rust-lint table with `unsafe_code` downgraded to `"deny"` (still fails by default; a local `#[allow(unsafe_code)]` permits exactly these four reviewed sites), while `[lints.clippy]` duplicates the workspace's clippy table unchanged (Cargo's `workspace = true` inherits a `[lints]` table wholesale or not at all -- no per-tool-group partial inheritance).
- Replace `derive_release_commit_decision`'s 2*N per-manifest `git show` spawns with 2 batched `git cat-file --batch` invocations.
  
  Verifying a merged release commit's claimed version roster looped over every workspace package's canonical manifests and, for each one, called `manifest_version_at` twice -- once against the parent commit, once against the release commit -- each shelling out a separate `git show <commit>:<path>` subprocess. For a monorepo with N canonical manifests that was 2N sequential subprocess spawns to verify one release commit, on top of the `git diff-tree` call already made earlier in the same function.
  
  Since only two distinct commits are ever queried, both are now fetched with exactly one `git cat-file --batch` invocation each: every requested manifest path is written to the child's stdin up front, and each object's blob content (or `missing`, for a manifest that didn't exist yet at the parent commit) streams back on stdout in one round trip. `CommandRunner` gains a new `run_with_stdin` method (default: `Unsupported`, so none of the many existing test-double/`moon` implementors need to change) that `CliCommandRunner` implements for real, piping input on a dedicated writer thread concurrently with draining the child's stdout/stderr so a payload larger than a pipe buffer can't deadlock.
  
  Because this feeds release-commit trust verification, the batch-output parser is deliberately defensive rather than permissive: each response is matched against its exact requested `commit:path` object string (not a generic pattern), a declared content length that doesn't fit the remaining output or isn't followed by the protocol's separator byte fails the whole batch closed, and any leftover or truncated output once every requested path is accounted for is rejected rather than ignored. A new regression test drives `derive_release_commit_decision` with a recording `CommandRunner` double across three canonical manifests and asserts exactly 2 `cat-file --batch` invocations occur, not 6.
- Replace release-commit verification's changeset re-derivation with persist-and-verify. `callisto version --emit-decision <path>` now writes the exact release decision (package, target version, inclusion reason) it computed, alongside the manifest and changelog edits it already writes. `callisto release plan --from-release-commit` (now requiring a companion `--decision <path>`) verifies the merged commit's actual diff against that committed decision instead of re-deriving changeset, fixed-group, linked-group, cascade, and pre-release-policy inclusion from raw git history a second time.
  
  The prior re-derivation only understood a direct changeset-to-package match: it rejected any real release where a fixed-group cascade bumped a sibling package the deleted changeset never named, which is every release in a fixed-group workspace once more than one package versions together. Trusting the already-computed decision, tamper-checked via its own content digest, removes that entire reimplementation and the drift it was exposed to.
- **`GraphError::ReleaseIntentStale` (E124) becomes a struct variant carrying a real reason**
  
  PR1 of a 4-PR migration (SPEC-ARCH-RELEASE-ERROR-TAXONOMY) fixing a confirmed architectural defect: `ReleaseIntentStale` was a fieldless unit variant constructed at ~95 sites across `commands/{release.rs,release_decision.rs,release_execution.rs}`, of which only 6 were genuine staleness -- the rest silently discarded an already-typed failure cause (a model error, a subprocess exit status, a merged-commit verification mismatch, a remote conflict, and more) behind the same generic, sometimes misleading "no release operation was authorized" message.
  
  - `ReleaseIntentStale` is now `{ reason: StaleReason }`; its Display message appends the reason, and its diagnostic code/help text are unchanged. This is semver-breaking for any external consumer pattern-matching the old unit variant (none exist inside this workspace).
  - `StaleReason` (`callisto-graph::commands::release`) is nameable everywhere but constructible with a real reason only from within `release.rs` itself -- `trust_evidence_changed`, `source_identity_changed`, `git_remote_changed`, and `intent_differs_from_fresh_derivation` cover the module's 6 genuine fresh-re-observation sites. Every other prior `ReleaseIntentStale` site (44 in `release.rs`, 42 in `release_decision.rs`, 2 in `release_execution.rs`) now uses a temporary, `#[deprecated]`, `pub(crate)` `StaleReason::legacy_unclassified()` migration ratchet, pinned exactly by a new architecture test and lowered to zero across PR2-PR4 as each site is given its real cause.
  - Twelve new `GraphError` variants (E158-E171) give a typed home to every previously-discarded cause -- model construction errors, registry failures, subprocess command failures, merged-commit verification mismatches, remote conflicts, unsupported feature combinations, invalid selections, unmet preconditions, and internal invariants -- ready for PR2-PR4 to wire into their real call sites.
  - New architecture tests in `crates/callisto-graph/tests/architecture.rs` enforce the module-privacy boundary, pin the ratchet counts, guard against new `map_err(|_ident| ...)` source-discarding closures elsewhere in the crate, and confirm E124's "reapprove" guidance isn't copied onto any of the new variants.
  
  No behavior change beyond E124's Display text gaining a reason suffix.
- **`release_execution.rs` reports its real artifact-manifest failure instead of a generic stale-intent error**
  
  PR2 of the 4-PR `SPEC-ARCH-RELEASE-ERROR-TAXONOMY` migration (PR1: #79). `execute_release_with_artifacts`'s two `GraphError::ReleaseIntentStale` sites now report their real cause: a manifest that fails `validate_for_intent` surfaces the existing `GraphError::ArtifactManifest { source }` (E136) with the specific `ArtifactManifestError` cause (duplicate slot, mismatched slot roster, mismatched attestation, mismatched intent); an intent that declares artifact slots but receives no manifest at all surfaces `GraphError::ReleasePreconditionUnmet { requirement: ArtifactManifestProvided }` (E170). Extracted the decision into `require_artifact_manifest_matches_intent` for direct unit coverage (previously untested). The crate's `legacy_unclassified_ratchet` architecture test is updated: `release_execution.rs` is now at 0 (was 2), and the file is removed from the `map_err_ignore` evasion allowlist.
- **`release_decision.rs` reports its real failure cause instead of a generic stale-intent error**
  
  PR3 of the 4-PR `SPEC-ARCH-RELEASE-ERROR-TAXONOMY` migration (PR1: #79, PR2: #80). All 42 `StaleReason::legacy_unclassified()` sites in `commands/release_decision.rs` now report a real, specific `GraphError` cause instead of the fieldless "no release operation was authorized" message:
  
  - **Subprocess exits/output** (`git rev-parse`, `diff-tree`, `cat-file --batch`, name-status parsing, the batch cat-file protocol response): `GraphError::ReleaseCommand { program, args, failure }` (E164) -- `CommandFailure::NonZeroExit { exit_code, stderr }` for real non-zero exits, `CommandFailure::MalformedOutput { detail }` for framing/parsing failures (wrong object type, missing separator, size overflow, non-hex sha, missing manifest version, etc). Two local helpers (`command_non_zero_exit`, `command_malformed_output`), scoped to this file only, build these consistently.
  - **Merged-commit verification** (`derive_release_commit_decision`): each of the 8 distinct checks now reports its own `GraphError::ReleaseCommitVerificationFailed { commit, reason }` (E165) variant -- `HeadMismatch`, `DecisionNotWrittenByCommit`, `NoChangesetConsumed`, `ChangelogNotTouched`, `ManifestNotInDiff`, `ManifestVersionMismatch`, `UnclaimedVersionChange`, `ClaimedPackageNotObserved` -- rather than all eight collapsing into the same message. The prior combined `!changed_version || after != target_version || !changed_paths.contains(path)` check is split into two ordered checks (`ManifestNotInDiff` then `ManifestVersionMismatch`) with identical overall pass/fail behavior.
  - **Corrupt committed decision file**: `serde_json::from_str` failure on the decision blob now reports `GraphError::ReleaseDecisionDecode { path, message }` (E166), distinct from every commit-verification mismatch.
  - **A claimed package with no changelog configured**: `GraphError::ReleasePreconditionUnmet { requirement: ChangelogConfigured }` (E170).
  - **Unhandled `BumpReason` variant** (`#[non_exhaustive]`): `GraphError::UnsupportedRelease { feature: BumpReason }` (E168).
  - **Duplicate/out-of-plan `--package` selections** in `derive_selected_release_decision`: `GraphError::ReleaseSelectionInvalid { package, reason }` (E169) naming the offending package (`Duplicate`/`NotInPlan`), rather than a generic failure with no package identified.
  - **Already-typed causes previously discarded by `map_err(|_error| ...)`**: `ReleasePackageId::new`/`ReleaseDecisionV1::new`/`Version::parse` failures now pass their real `#[from]` source through (`ReleasePackageId` E162, `ReleaseDecision` E160, `VersionParse`), instead of being replaced with the generic stale-intent message.
  - **Internal invariants** (a version-plan bump referencing a package absent from the same workspace observation, a cascade/peer-escalation `via` package with no unique release identity, an ecosystem with no canonical manifest format reached from a canonical manifest): `GraphError::ReleaseInvariant { detail }` (E171).
  
  Every prior site was classified; none were left on `legacy_unclassified()`. The crate's `legacy_unclassified_ratchet` architecture test is updated: `release_decision.rs` is now at 0 (was 42), and the file is removed from the `map_err_ignore` evasion allowlist (zero `map_err(|_ident| ...)` discards remain in it). Existing tests pattern-matching the old `ReleaseIntentStale` shape at these sites are updated to the new variants; new regression tests cover the most safety-critical distinctions (`HeadMismatch`, corrupt decision file, `ClaimedPackageNotObserved`, duplicate/not-in-plan selections).
- **`release.rs` reports its real failure cause instead of a generic stale-intent error (final PR of the migration)**
  
  PR4 of the 4-PR `SPEC-ARCH-RELEASE-ERROR-TAXONOMY` migration (PR1: #79, PR2: #80; PR3 migrates `release_decision.rs` separately). All 44 `StaleReason::legacy_unclassified()` sites in `commands/release.rs` -- registry publish dispatch, tag/forge dispatch, and release-intent/operation derivation -- now report a specific `GraphError` cause instead of the generic "no release operation was authorized" message:
  
  - Registry classification failures (`classify_cargo_output`/`classify_npm_publish_output`/`classify_twine_output`) now surface `GraphError::Registry { package, source }` (E163), carrying the real `RegistryError::{AuthFailed,RateLimited,Network,Other}`.
  - Non-zero subprocess exits and malformed output for `git`/`gh`/`npm` (push, tag observation, `for-each-ref`, `gh release create`, `gh api`, `npm view`, the pypi build step) now surface `GraphError::ReleaseCommand { program, args, failure }` (E164) with `CommandFailure::NonZeroExit`/`MalformedOutput`.
  - Remote-object conflicts observed after an authorized effect (a tag at a different commit, a tag not observed after push, a forge release that already differs or wasn't observed after creation, an unexpected forge API status) now surface `GraphError::ReleaseRemoteConflict { conflict }` (E167), using all five `RemoteConflict` variants.
  - Unsupported ecosystem/source-identity/publish-target combinations now surface `GraphError::UnsupportedRelease { feature }` (E168).
  - A registry publish target that would collapse two operations into one now surfaces `GraphError::ReleaseSelectionInvalid { package, reason: DuplicateRegistryTarget }` (E169).
  - Unmet preconditions (a non-detached HEAD, a canonical root that doesn't match the workspace root, no git remote prepared, no GitHub remote configured) now surface `GraphError::ReleasePreconditionUnmet { requirement }` (E170).
  - Internal invariants (an unknown prepared-operation id, a `PreparedOperation`/pypi-argv shape mismatch, a for-each-ref line with no annotation field, a package with no canonical manifest, an operation-order cycle, a publish target with no registry key) now surface `GraphError::ReleaseInvariant { detail }` (E171).
  - Model-constructor failures (`ReleaseIntentV1::new`, `ReleaseOperation::{registry_publish,new,tag,forge_release}`, `RegistryBindingId::new`, `ReleaseInputSnapshotV1::new`) now propagate via `?` through the existing `#[from]` conversions (E158/E159/E161) instead of being discarded.
  - Three pre-existing, StaleReason-unrelated `map_err(|_error| ...)` discards in `canonical_registry_binding`/`canonical_git_remote` (URL and GitHub-repository parse failures) now classify the real error into a specific static reason instead of a single generic one.
  
  `crates/callisto-graph/tests/architecture.rs`: `legacy_unclassified_ratchet`'s `commands/release.rs` count is lowered from 44 to 0, and `commands/release.rs` is removed from `MAP_ERR_IGNORE_ALLOWLIST` entirely (zero `map_err(|_ident| ...)` discards remain in the file). The `#[deprecated]` `StaleReason::legacy_unclassified()` constructor itself stays defined -- `commands/release_decision.rs` (PR3, landing separately) still uses it -- and will be deleted once that file also reaches zero.
  
  No behavior change beyond error messages/codes becoming specific to their real cause.
- The mapping from a package's canonical manifests to ecosystem-qualified `ReleasePackageId`s was independently reimplemented at three sites: `derive_release_decision`, `derive_release_commit_decision` (which recomputed it a second time within the same function, from values it had already derived once), and `release::derive_release_inputs` (via a differently-shaped `BTreeSet<Ecosystem>` dedup step). `derive_release_decision` and `derive_release_commit_decision` are the two ends of the same trust boundary -- one computes the release roster, the other verifies it -- so drift between their derivations was a real risk to that boundary's guarantee.
  
  Added `release_decision::release_package_ids` as the single derivation, used by all three sites; deleted the redundant in-function recomputation in `derive_release_commit_decision`. No behavior change for any real caller.
- Fix `changed_since_last_tag` walking the whole repo instead of a package's own paths. It called `commits_since(Some(&tag), &[])` with an empty pathspec, so any commit anywhere in the workspace since the package's tag short-circuited `Ok(true)` before the path-scoped `git diff --quiet <tag> -- <paths>` check a few lines below ever ran. In an active monorepo this made the function return `true` for nearly every package on nearly every call, since some other package almost always committed since -- `status`'s `changed_since_last_tag` field was effectively meaningless. `commits_since` is now scoped to `package_paths(pkg)`, identically to the diff fallback; an empty scoped result still falls through to the `git diff --quiet` check, which alone catches uncommitted working-tree changes.
- **Add a shared canonical-manifest-identity reader; stop hand-parsing Cargo.toml/package.json/pyproject.toml independently in six places**
  
  `callisto-model` gains `Ecosystem::CANONICAL` (`[Cargo, Npm, Pypi]`) and `Ecosystem::canonical_manifest_format()`, the single enumeration of "which ecosystems have a canonical identity manifest, and which format is it" -- replacing hand-written `if root.join("Cargo.toml").exists() { ... } else if ...` chains.
  
  `callisto-manifests` gains `read_identity(format, source, path) -> Result<ManifestIdentity, ManifestError>`, a pure, I/O-free reader for callers that already hold a manifest's content as a string (a `git show` blob, a directory walker's pre-read buffer) instead of a path `Manifest::open` can read from disk. `ManifestIdentity { name, version }` carries `version` as a new `VersionSource` enum (`Literal(String)` vs `InheritedFromWorkspace`) rather than collapsing Cargo's `version.workspace = true` into `None`. Also adds `read_napi_targets(path, &Value) -> Result<Option<Vec<String>>, ManifestError>`, the one shared parser for `napi.targets`.
  
  Six call sites in `callisto-graph`/`callisto-moon` now route through these instead of re-implementing the same extraction: `IdentityResolver::resolve`, `IgnoreWalkLocator::projects`, `MoonProjectLocator`'s ecosystem detection, workspace-membership manifest-filename lookups, `NapiTargetsIndex::load`, and `matrix::read_napi_targets`.
  
  Two real behavior changes fall out of this:
  
  - **`IgnoreWalkLocator::projects()` now discovers Flit-based Python packages.** Its own pyproject.toml parsing previously only checked `project.name` and `tool.poetry.name`, missing the `tool.flit.metadata.module` fallback the shared extractor already had -- a Flit package was silently undiscovered before this change.
  - **`NapiTargetsIndex::load` and `matrix::read_napi_targets` now share one parser with their policy difference visible at the call site**: both still disagree on how to handle a malformed `napi.targets` (`NapiTargetsIndex::load` stays lenient via `.ok()`, `matrix::read_napi_targets` stays strict by propagating the error), but a `napi.targets` array containing a non-string entry now causes `NapiTargetsIndex::load` to drop the whole array instead of silently filtering out just the bad entry -- a narrower form of the same lenient policy, no longer a second independent implementation.
  
  `GraphError`'s `manifest_version_at` (release-commit verification against a historical git blob) also now goes through `read_identity` instead of its own inline `toml_edit`/`serde_json` parsing.
- `create_tags_with_options`'s preview branch (`permit: None`) and write branch (`permit: Some`) each independently computed `contains_tag` + `resolve_commit` + `RefNotFound` for an already-existing tag's actual sha. Extracted `resolve_existing_or_release_sha` as the single implementation, alongside the file's existing `plan_floating_major` helper -- which solved the identical preview/write divergence risk for the floating-major case. Both branches now call it instead of duplicating the resolution logic. No behavior change.
- **Fix `TagIndex::build` re-deriving a package's default tag template instead of reusing `TagTemplate::default_for`**
  
  `TagIndex::build`'s per-package loop built each package's default tag template by hand-formatting `"{name}@{version}"` and re-parsing it via `TagTemplate::parse`, instead of calling `callisto_model::TagTemplate::default_for` -- the single source of truth for this exact value already used by `release.rs`'s own tag-name construction (`unwrap_or_else(|| callisto_model::TagTemplate::default_for(&package.id))`). The two implementations behaved differently: `TagTemplate::parse` additionally validates git-ref-name legality, so `TagIndex::build` could reject a package name that `release.rs`'s path would accept unchecked -- genuine behavioral divergence between two code paths computing the same value.
  
  `TagIndex::build` now calls `TagTemplate::default_for` directly, matching `release.rs`. **Behavior change**: this intentionally removes the extra git-ref-name validation `TagTemplate::parse` was performing on this call site's default-template path. This is not a silent regression -- it aligns `tags.rs` with the already-shipped `default_for` behavior `release.rs` relies on, rather than adding a new validation requirement to `default_for` itself (which would affect every caller). A custom `tag_template` configured via `[[package-set]]` still goes through `TagTemplate::parse` and its full validation, unchanged.
- **Parse GitHub repository and Git tag identity instead of validating ad hoc (audit pattern F)**
  
  `callisto-model` gains two real identity types replacing several independent, inconsistent ad hoc validations of the same concepts:
  
  - `GitHubRepository::parse` replaces three separate `owner/repo` checks: `GitHubAttestationPolicyV1`'s `is_safe_github_repository` (a bare `split_once('/')` with no character-class check at all), `release_pr.rs`'s `validate_repository`/`valid_repo_part` (ASCII alphanumeric plus `-_.` per part), and an entirely unvalidated `format!("{owner}/{repository}")` built from a parsed Git remote URL in `callisto-graph`. Every caller now parses through one charset rule: both `owner` and `repo` must be non-empty, ASCII alphanumeric plus `-`, `_`, `.`, and must not start or end with `-`. `GitHubAttestationPolicyV1::repository`, `GitHubArtifactAttestationV1::repository`, `ArtifactSlotId::new`'s `repository` parameter, `ReleasePrConfigV1::repository`, `ReleasePrSnapshotV2::repository`, and `ReleasePrPullRequestV2::head_repository` all change from `String` to `GitHubRepository`; it serializes and deserializes as its plain `"owner/repo"` string form (matching `ReleasePackageId`'s existing convention), so no wire-format change.
  
    **Behavior change**: this is a real tightening at a security-sensitive validation boundary (`GitHubAttestationPolicyV1` gates the `gh attestation verify --repo <value>` provenance check), not just an internal refactor. A value that previously passed `is_safe_github_repository` can now be rejected -- concretely, embedded whitespace (for example `"owner name/repo"`), a leading or trailing `-` in either part, or more than one `/`.
  
  - `TagName`'s public `String` field is now private. `TagName::parse` rejects a leading `-` (which `git`/`gh` argument parsers read as a flag, even though it is a legal Git ref character) plus anything `git check-ref-format` would reject (control characters, `..`, `@{`, a `.lock`-suffixed or leading-`.` ref component, and Git's other reserved characters). `TagName::new_unchecked` remains as a documented escape hatch for the few call sites (tag-template rendering, last-tag selection) that can prove their input is already constrained to a trusted, non-`-`-leading charset. `callisto-vcs`'s `list_tags` (both the native and shelled-out backends) now silently skips any existing repository tag ref that fails this check instead of returning it uncritically -- an existing Git ref is always legal, but not necessarily safe to hand to a CLI parser as a bare positional.
  
  `callisto-graph`'s `canonical_git_remote` now rejects a `github.com` remote whose derived owner/repository fails `GitHubRepository::parse` (as `GraphError::UnsafeGitRemote`) instead of forwarding an unvalidated string to `dispatch_forge_release`. `callisto-cli`'s `release-pr decide --repository` now rejects a leading/trailing `-` in the owner or repository name that the prior, slightly looser check accepted.
- **Batch `WorkspaceCargoResolver`-routed writes to a shared root `Cargo.toml` into one open/mutate/persist cycle**
  
  `WorkspaceCargoResolver::write_version`/`write_dependency` in `callisto-manifests` no longer call `persist` internally -- they now do pure in-memory mutation only, mirroring the `Manifest` trait's `write_version`/`update_dependency_spec`/`persist` split already used by `CargoToml`/`PackageJson`/`PyprojectToml`. `persist` is promoted to a public, explicitly-called method. Every existing call site (the inherited-dependency delegation inside `CargoToml::update_dependency_spec`, and both of `apply_version_plan`'s per-write fallback loops) now calls `persist` explicitly afterward -- no behavior change there.
  
  `callisto-graph`'s `classify_manifest_writes`/`apply_version_plan` (`apply.rs`) now route root-manifest writes (`VersionWriteTarget::CargoWorkspacePackage`/`DepWriteTarget::CargoWorkspaceDependency`) into a new `resolver_batched` group when the same physical root manifest receives no competing write through the `Manifest` trait. A release that bumps the workspace version plus several workspace-inherited dependencies now opens the root `Cargo.toml` once, applies every mutation in memory, and persists exactly once -- down from one read+parse+atomic-write per write. The pre-existing mixed-routing exclusion (a root manifest written through *both* the `Manifest` trait and `WorkspaceCargoResolver` -- the data-loss/ordering hazard `classify_manifest_writes` already guarded against) is unchanged: those paths still get no batching on either side, processed individually and in the same order as before.
  
  New `resolver_load_call_count`/`resolver_persist_call_count` test-observability counters (kept separate from `open_call_count`/`persist_call_count`, since `OpenContext::for_workspace_root` calls `WorkspaceCargoResolver::load` unconditionally whenever a workspace root `Cargo.toml` exists, which would otherwise pollute those counters workspace-wide) prove the new batched path opens and persists the shared root manifest exactly once per `apply_version_plan` call, regardless of how many bumps/rewrites target it, while the mixed-routing case continues to persist each write individually.

## 0.6.0

- **Add `callisto filter-plan` and the primitives it's built on**
  
  New `callisto filter-plan --plan <plan> --report <report>` filters a publish plan down to what a publish report confirms actually succeeded, dropping anything that failed. Lets a release pipeline run `plan-publish` -> `publish` -> `tag`/`gh release create` as separate steps and have the last two operate on what actually shipped, instead of the pre-publish plan.
  
  Built on two new, additive primitives: `PublishPlan::is_empty()`, and `CreatedTag.isFloatingMajor` (distinguishes a floating major-version alias from an immutable per-version release tag in `callisto tag`'s output). Both are backward-compatible — no existing command's behavior changes.
- **Fix: skip an npm main package when its platform dependency fails to publish**
  
  Previously an npm main package would still publish even when a declared platform dependency (`optionalDependencies`) failed in the same run, shipping a version that referenced an unpublished package. The dependency-failure check is scoped to the npm ecosystem, so a same-named Cargo crate failure can no longer false-positive-match an unrelated npm platform dependency.
- **Fix: `--package` ecosystem collisions and unvalidated platform dependencies**
  
  - `--package` now resolves names by ecosystem-aware identity instead of bare string. A Cargo crate and an npm package sharing a name no longer both match one `--package` request; a genuinely ambiguous bare name now errors instead of silently including both (qualify it with `npm:name` to disambiguate).
  - `depends_on_platforms` is now validated against the final plan. A main npm package whose platform dependency is missing — misconfigured, or excluded by `--package` — now fails the plan instead of publishing a broken `optionalDependencies` reference.
  - `--package` naming a real package with nothing pending now returns a precise reason (not a release candidate, or no dispatchable publish target) instead of the generic "unknown package" message.
- **Fix: a dev-only publish cycle no longer un-orders unrelated dev-dependencies**
  
  `publish_order` previously dropped dev-dependency ordering for the entire publish batch the moment any one legitimate dev-only cycle existed (e.g. two crates mutually dev-depending on each other for cross-integration tests). The exclusion is now scoped to just the cyclic pair — every other dev-dependency ordering in the same batch is still honored.
- **Add `--skip-publish-precheck` to skip the redundant already-published registry check**
  
  `callisto publish` previously called the registry's `is_published` check before every publish attempt, even though a fresh publish always returns false there and a conflicting publish is already correctly classified as `AlreadyPublished` from the publish call itself. Pass `--skip-publish-precheck` to skip that extra round-trip; the default behavior is unchanged.
- Released together with the `workspace` fixed group.

## 0.5.0

- # Native artifact placement for CI-built platform binaries
  
  - The shipped GitHub Action (`callisto-action`) now downloads each `callisto matrix`-driven native build's CI artifact and places it into its package directory automatically before `callisto publish` runs -- closing the gap between the build matrix and a working end-to-end release for napi-rs/maturin platform packages. No consumer-authored placement step is needed; it happens whenever `nativeMatrix` is non-empty and `publish` is enabled.
  - Fixed: two packages sharing the same target platform triple no longer collapse into one `artifactName`, which previously dropped one package's build from the release silently.
  - `callisto matrix --format json`'s `artifactName` values now follow napi-rs's own `<name>-<platform>-<arch>[-<abi>]` convention (previously the raw Rust target triple), and scoped npm package names such as `@scope/addon` no longer produce an `artifactName` containing `/`, which `actions/upload-artifact` rejects.

## 0.4.1

- **Publish order now accounts for dev-dependencies between same-batch packages**
  
  - **Fixed:** `cargo publish` (run without `--no-verify`) rebuilds the packaged tarball to verify it, which needs every declared dependency -- including `[dev-dependencies]` -- resolvable from the registry. Publish ordering previously ignored dev-dependency edges entirely, so a package with a dev-dependency on a same-batch sibling could be published before that sibling, failing with an unresolvable version requirement.
  - Dev-dependency ordering is best-effort, not a hard requirement: a legitimate mutual dev-dependency between two otherwise-unrelated packages no longer risks hard-failing the whole publish plan -- it falls back to the previous (non-dev) ordering for that case instead.

## 0.4.0

- **New `callisto matrix` command**
  
  - Discovers napi and maturin platform targets from `package.json`/`pyproject.toml`.
  - Builds a per-triple CI table: host runner, cross-compile flag, artifact name.
  - Reports `engines.node`/`requires-python` versions.
- **Cascade correctness: peer-escalation severity and cross-ecosystem rewrite keys**
  
  - A dependent package no longer gets over-escalated to a major/minor bump when the upstream change was actually a patch.
  - Publishing a package that exists in both Cargo and npm no longer crashes during version bumps with a "dependency not found" error.
- **One changelog renderer, everywhere**
  
  - PR descriptions and `CHANGELOG.md` entries are now generated by the same logic, so they no longer disagree.
  - Bumps inferred from commit messages, and packages newly added to a group, now get a real changelog entry instead of a placeholder.
  - If several changes caused one package's bump, the changelog now lists all of them, not just one.
- **Fixed groups converge on a single, shared target version**
  
  - Packages in a `[[fixed-group]]` now bump to the same version together, instead of drifting apart when they carry changesets of different severity.
  - A group that would end up in an inconsistent state now aborts with a clear error instead of silently shipping a broken version.
  - Two differently-spelled group-member entries that actually point to the same package are now caught as a config error instead of silently accepted.
- **Git-access layer decoupling, performance, and small correctness fixes**
  
  - **Breaking (library consumers only):** `callisto-conventional`'s public functions no longer take a `callisto-vcs` type directly.
  - A `BREAKING CHANGE:` commit footer is now parsed correctly.
  - Commits merged in from another branch are no longer missed when scanning history since the last tag.
  - `^1.2.3` no longer incorrectly matches a prerelease like `1.9.0-alpha.1`.
  - Compound Cargo dependency ranges are now rewritten correctly during a version bump.
  - PEP 440 prerelease detection and version-range matching for Python packages is now correct.
  - Python dependency names are now matched with PEP 503 normalization (e.g. `My-Package` and `my_package` are treated as the same dependency).
  - npm publish now auto-detects your workspace's package manager (pnpm/yarn/bun/npm) instead of assuming npm.
  - A few errors that used to share one error code now have their own, more specific code.
  - `status`, `plan-snapshot`, `plan-publish`, and `plan-version` are now faster on large repos — the git repository is no longer rediscovered per package.
  - Tag-existence checks now use a cached index instead of one lookup per release.
- **Better cross-ecosystem package discovery**
  
  - Packages excluded from your Cargo/npm/pnpm workspace (fuzz targets, scratch examples) no longer show up as release-managed.
  - A Rust crate and its same-named Python binding in one workspace no longer trigger a duplicate-package error.
  - `cargo:name`/`npm:name` prefixes in group config now work as documented.
  - A package name is no longer silently matched to the wrong ecosystem when the intended match is missing.
- **Manifest writes are batched and more reliable**
  
  - **Breaking (custom manifest integrations only):** implementing callisto's `Manifest` trait now requires a `persist()` method to actually write your changes.
  - Re-running a version bump is now safe and won't risk losing a write.
  - Multiple changes to the same manifest file in one run are now applied together instead of risking one overwriting another.
- **Platform-manifest and `optionalDependencies` write planning**
  
  - Fixed groups with platform-specific packages (napi/maturin) now get their platform manifests and `optionalDependencies` updated automatically during a version bump — previously nothing happened.
- **npm publish access modeled as a 3-state enum**
  
  - **Breaking:** `plan-publish`'s npm target now reports `access` (`"public"`/`"restricted"`/unset) instead of a `restricted: bool` — update anything parsing that JSON field.
  - An unscoped npm package with `publishConfig.access: "public"` in its `package.json` is no longer silently dropped during publish planning.
- **Release-pipeline / CI Action contract correctness**
  
  - **Breaking:** several `--format json` field names changed or were added (`validate`, `compose-pr-body`, `tag`, `status`, `plan-publish`) — update any scripts parsing this output.
  - Re-tagging a release that already exists no longer reports the wrong commit sha.
  - The official GitHub Action now actually opens a release PR when changesets are pending — a bug made this step unreachable before.
  - GitHub Releases are now correctly marked prerelease for PEP 440 versions too (e.g. `1.2.3a1`), not just SemVer's `-` syntax.
  - Release notes now include the real changelog section instead of nothing.
- **Security hardening across publish, git, and subprocess handling**
  
  - **Breaking:** a package name starting with `-` is now reported as its own error, instead of being misreported as a path-traversal error.
  - A malicious package name can no longer inject extra flags into the underlying `cargo publish`/`npm publish`/`pypi publish` command.
  - A malicious `publishConfig.registry` URL can no longer redirect an npm publish to an unapproved registry.
  - An absolute or `..`-containing `changesets.dir`/changelog path in `callisto.toml` is now rejected instead of allowing writes outside the workspace.
  - Credentials no longer leak into error messages from failed git or registry-CLI commands.
  - A runaway subprocess can no longer exhaust memory via unbounded output capture, or hang a command indefinitely.
  - A tag name starting with `-` is now rejected, closing a git argument-injection hole.

## 0.3.3

### Patch Changes

- # Security hardening, correctness fixes, and performance improvements
  
  ## Security
  
  - **Argument injection in publish client** (`callisto-graph`): Package names are now validated before being passed to `cargo publish`, `npm publish`, and `pypi publish`. Names beginning with `-` are rejected with an error, preventing a crafted package name from injecting flags into the subprocess.
  - **Path traversal in `changesets.dir` config** (`callisto-graph`): The `changesets.dir` value from `callisto.toml` is now validated to contain no `..` components. A value like `../../tmp` previously allowed all changeset read and write operations to escape the workspace root.
  
  ## Correctness
  
  - **Changelog idempotency on stable release after pre-release** (`callisto-changelog`): The duplicate-entry guard in `prepend()` used a substring check that incorrectly matched `## 1.0.0-alpha.1` when writing `## 1.0.0`, permanently suppressing the stable release section. Now uses an exact line-boundary match.
  - **Empty changelog section headings** (`callisto-changelog`): `### Patch Changes` (and equivalent headings) are no longer emitted when every entry in the section has a blank summary.
  - **`pre-major-inference` config field now applied** (`callisto-graph`): The per-package `pre-major-inference` setting in `callisto.toml` was parsed and stored but never consulted. `aggregate()` hardcoded `OFF` for every package. The configured policy is now applied when constructing the inference window.
  - **Inference errors now surfaced as diagnostics** (`callisto-graph`): When `SeverityInference::infer` returns an error (e.g. due to a git failure), the error was previously swallowed with no diagnostic emitted and no bump recorded. The affected package now receives a warning-level diagnostic describing the failure.
  - **Ambiguous bare package name now errors in `status`** (`callisto-graph`): In a polyglot workspace containing both `cargo/foo` and `npm/foo`, a changeset entry naming bare `foo` was silently applied to every matching package. `status()` now returns `GraphError::AmbiguousName` with the full candidate list.
  - **Version overflow returns error instead of wrapping** (`callisto-format`): Incrementing a version component equal to `u64::MAX` previously wrapped to `0` in release builds (producing a version lower than the input) and panicked in debug builds. All bump paths now use `checked_add(1)` and return `BumpError::Overflow`.
  - **Decor preserved on full-table dependency form** (`callisto-manifests`): `CargoToml::update_dependency_spec` did not clone and reapply surrounding decor when the dependency used the `[dependencies.name]` full-table form, causing trailing comments and formatting to be lost on the first version bump.
  
  ## Performance
  
  - **O(N²) package lookups eliminated in publish and version commands** (`callisto-graph`): `plan_publish` and `plan_version` called `packages().find()` inside loops over package IDs and severity entries, giving O(N²) complexity. A single `HashMap<&PackageId, &Package>` is now built once before each loop.
  - **O(N) toposort validation** (`callisto-graph`): The toposort subset-validation loop called `all_packages.contains(id)` (O(N) slice scan) per member. A `HashSet` built once before the loop reduces this to O(N) total.
  - **Eliminated intermediate allocations in graph resolver** (`callisto-graph`): `dependencies_of()` and `dependents_of()` allocated a `Vec<&DepEdge>` and immediately called `into_iter()`. Both now return the iterator directly via `flat_map`.
  - **Unique changeset slugs without clock dependency** (`callisto-cli`): `generate_human_slug` derived all three word indices from a single nanosecond timestamp, yielding at most 8,000 distinct values. Concurrent `callisto add` calls on systems with coarse clock resolution could produce the same slug, causing `atomic_write` to silently overwrite the earlier changeset. Slugs now incorporate a process-global atomic counter for guaranteed uniqueness.
  
  ## Housekeeping
  
  - Removed unreachable `calculate_bump_severity` function from `cascade.rs` that was suppressed with `#[allow(dead_code)]`.

## 0.3.2

### Patch Changes

- Release update

## 0.3.0

### Minor Changes

- Release update

