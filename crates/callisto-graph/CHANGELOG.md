# callisto-graph

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

