# callisto-manifests

## 0.7.2

- Released together with the `workspace` fixed group.

## 0.7.1

- Released together with the `workspace` fixed group.

## 0.7.0

- `CargoToml::publish_targets()` was a byte-for-byte duplicate of the `Manifest` trait's own default `publish_targets()` implementation -- `CargoToml::ecosystem()` already returns `Ecosystem::Cargo`, so the default impl dispatches to the identical `CratesIo`/`None` output without an override. Deleted the redundant override; added a direct regression test (`publish_targets_uses_trait_default_dispatch_for_cargo`) pinning that the trait-default dispatch still produces the same output for both publishable and `publish = false` manifests. No behavior change.
- **Fix: `Cargo.toml` with CRLF line endings was silently rewritten to LF on any write**
  
  `npm.rs`'s `PackageJson` and `python.rs`'s `PyprojectToml` each independently detected and reapplied a file's original line-ending style (`LineEnding::{Lf,CrLf}`, `content.contains("\r\n")` on open, `.replace("\r\n", "\n").replace('\n', "\r\n")` on persist) alongside UTF-8 BOM handling -- but `cargo.rs`'s `CargoToml` only ever tracked `has_bom`, with zero line-ending handling. Any `Cargo.toml` with CRLF line endings (common on Windows / repos with `core.autocrlf=true`) got silently rewritten to LF on the next `write_version`/`update_dependency_spec` + `persist`, unlike the equivalent `package.json`/`pyproject.toml` in the same repo.
  
  Added a shared `common::FormatFingerprint { has_bom, line_ending }` with `detect(content)` and `apply(rendered)`, migrated `npm.rs` and `python.rs` onto it (pure refactor -- their existing BOM/CRLF tests pass unchanged, plus a new direct CRLF regression test for `npm.rs`, which previously had none), and added the same fingerprinting to `CargoToml::open`/`persist` -- the actual fix. Added a CRLF `Cargo.toml` corpus fixture (`callisto_fixtures::corpus::cargo_toml_crlf_no_bom_sample`) and a regression test proving a CRLF `Cargo.toml`'s line endings survive a `write_version` + `persist` round trip.
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
- `cargo.rs`'s `set_scalar_preserving_decor` (look up an existing scalar, clone its decor, build the new value, reapply decor if present, else insert fresh) and `python.rs`'s local `set_with_decor` closure inside `PyprojectToml::write_version` were the same CST-editing algorithm, differing only in operating on `toml_edit::Table::insert` versus `toml_edit::Item` indexing.
  
  Lifted into a single shared `common::set_scalar_preserving_decor(table: &mut dyn toml_edit::TableLike, key, new_value)`, since `toml_edit::TableLike` is implemented by both `Table` (Cargo's `[package]`/`[workspace.package]` tables, which coerce to `&mut dyn TableLike` directly) and reachable from an `Item` via `Item::as_table_like_mut()` (pyproject.toml's `[project]`/`[tool.poetry]`/`[tool.flit.metadata]`, each held as an `Item`). Both `cargo::CargoToml::write_version`, `cargo::WorkspaceCargoResolver::write_version`, and `python::PyprojectToml::write_version` now call the shared implementation; the two local copies are deleted. No behavior change -- decor-preservation tests for both ecosystems pass unchanged.
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
- **Add a shared canonical-manifest-identity reader; stop hand-parsing Cargo.toml/package.json/pyproject.toml independently in six places**
  
  `callisto-model` gains `Ecosystem::CANONICAL` (`[Cargo, Npm, Pypi]`) and `Ecosystem::canonical_manifest_format()`, the single enumeration of "which ecosystems have a canonical identity manifest, and which format is it" -- replacing hand-written `if root.join("Cargo.toml").exists() { ... } else if ...` chains.
  
  `callisto-manifests` gains `read_identity(format, source, path) -> Result<ManifestIdentity, ManifestError>`, a pure, I/O-free reader for callers that already hold a manifest's content as a string (a `git show` blob, a directory walker's pre-read buffer) instead of a path `Manifest::open` can read from disk. `ManifestIdentity { name, version }` carries `version` as a new `VersionSource` enum (`Literal(String)` vs `InheritedFromWorkspace`) rather than collapsing Cargo's `version.workspace = true` into `None`. Also adds `read_napi_targets(path, &Value) -> Result<Option<Vec<String>>, ManifestError>`, the one shared parser for `napi.targets`.
  
  Six call sites in `callisto-graph`/`callisto-moon` now route through these instead of re-implementing the same extraction: `IdentityResolver::resolve`, `IgnoreWalkLocator::projects`, `MoonProjectLocator`'s ecosystem detection, workspace-membership manifest-filename lookups, `NapiTargetsIndex::load`, and `matrix::read_napi_targets`.
  
  Two real behavior changes fall out of this:
  
  - **`IgnoreWalkLocator::projects()` now discovers Flit-based Python packages.** Its own pyproject.toml parsing previously only checked `project.name` and `tool.poetry.name`, missing the `tool.flit.metadata.module` fallback the shared extractor already had -- a Flit package was silently undiscovered before this change.
  - **`NapiTargetsIndex::load` and `matrix::read_napi_targets` now share one parser with their policy difference visible at the call site**: both still disagree on how to handle a malformed `napi.targets` (`NapiTargetsIndex::load` stays lenient via `.ok()`, `matrix::read_napi_targets` stays strict by propagating the error), but a `napi.targets` array containing a non-string entry now causes `NapiTargetsIndex::load` to drop the whole array instead of silently filtering out just the bad entry -- a narrower form of the same lenient policy, no longer a second independent implementation.
  
  `GraphError`'s `manifest_version_at` (release-commit verification against a historical git blob) also now goes through `read_identity` instead of its own inline `toml_edit`/`serde_json` parsing.
- **Batch `WorkspaceCargoResolver`-routed writes to a shared root `Cargo.toml` into one open/mutate/persist cycle**
  
  `WorkspaceCargoResolver::write_version`/`write_dependency` in `callisto-manifests` no longer call `persist` internally -- they now do pure in-memory mutation only, mirroring the `Manifest` trait's `write_version`/`update_dependency_spec`/`persist` split already used by `CargoToml`/`PackageJson`/`PyprojectToml`. `persist` is promoted to a public, explicitly-called method. Every existing call site (the inherited-dependency delegation inside `CargoToml::update_dependency_spec`, and both of `apply_version_plan`'s per-write fallback loops) now calls `persist` explicitly afterward -- no behavior change there.
  
  `callisto-graph`'s `classify_manifest_writes`/`apply_version_plan` (`apply.rs`) now route root-manifest writes (`VersionWriteTarget::CargoWorkspacePackage`/`DepWriteTarget::CargoWorkspaceDependency`) into a new `resolver_batched` group when the same physical root manifest receives no competing write through the `Manifest` trait. A release that bumps the workspace version plus several workspace-inherited dependencies now opens the root `Cargo.toml` once, applies every mutation in memory, and persists exactly once -- down from one read+parse+atomic-write per write. The pre-existing mixed-routing exclusion (a root manifest written through *both* the `Manifest` trait and `WorkspaceCargoResolver` -- the data-loss/ordering hazard `classify_manifest_writes` already guarded against) is unchanged: those paths still get no batching on either side, processed individually and in the same order as before.
  
  New `resolver_load_call_count`/`resolver_persist_call_count` test-observability counters (kept separate from `open_call_count`/`persist_call_count`, since `OpenContext::for_workspace_root` calls `WorkspaceCargoResolver::load` unconditionally whenever a workspace root `Cargo.toml` exists, which would otherwise pollute those counters workspace-wide) prove the new batched path opens and persists the shared root manifest exactly once per `apply_version_plan` call, regardless of how many bumps/rewrites target it, while the mixed-routing case continues to persist each write individually.

## 0.6.0

- Released together with the `workspace` fixed group.

## 0.5.0

- Released together with the `workspace` fixed group.

## 0.4.1

- Released together with the `workspace` fixed group.

## 0.4.0

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
- **Manifest writes are batched and more reliable**
  
  - **Breaking (custom manifest integrations only):** implementing callisto's `Manifest` trait now requires a `persist()` method to actually write your changes.
  - Re-running a version bump is now safe and won't risk losing a write.
  - Multiple changes to the same manifest file in one run are now applied together instead of risking one overwriting another.
- **npm publish access modeled as a 3-state enum**
  
  - **Breaking:** `plan-publish`'s npm target now reports `access` (`"public"`/`"restricted"`/unset) instead of a `restricted: bool` — update anything parsing that JSON field.
  - An unscoped npm package with `publishConfig.access: "public"` in its `package.json` is no longer silently dropped during publish planning.

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

