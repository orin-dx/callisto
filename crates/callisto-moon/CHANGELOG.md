# callisto-moon

## 0.7.0

- `IdentityResolver::resolve` did a fresh `fs::read_to_string` + full manifest parse on every call with no memoization. `MoonProjectLocator::declared_edges` re-resolved every project's identity a second time (`projects()`, called earlier in the same flow, already resolved it once) and, worse, re-resolved a dependency edge's target once per edge rather than once per unique target project -- a widely-depended-on package (e.g. a shared crate with 30 internal dependents) had its manifest read and parsed 30+ times in a single `Workspace::load`.
  
  `IdentityResolver` now memoizes `resolve`'s result by `(path, ecosystem)` for its whole lifetime. Since `MoonProjectLocator` holds one `IdentityResolver` across both `projects()` and `declared_edges()`, a given manifest is now read and parsed at most once per run, with no change needed to `declared_edges` itself.
- **Deduplicate `execute_extension`'s five error-response branches into one helper**
  
  `execute_extension` (`extension_pdk.rs`) had five failure branches -- locator error, `Workspace::load` error, `plan_publish` error, `validate` error, `status` error -- each hand-building the identical `ExecuteExtensionOutput { report: json_val.clone(), rendered: e.to_string(), exit_code: 1 }` struct literal, most after calling `format_graph_error_json(&e)`. Extracted the shared shape into `extension::error_output`, called from all five sites. The `.clone()` on `json_val` was pointless in every branch -- `json_val` is never read again after being moved into `report` -- so the helper takes ownership directly instead.
  
  No behavior change.
- `MoonCommandRunner::run`'s native (non-`pdk`) path now classifies a spawn failure as `CommandError::NotFound` by checking the real `io::Error`'s `ErrorKind::NotFound` directly, instead of stringifying the error first and pattern-matching "not found"/"no such file" substrings in its `Display` text. That message-text heuristic (still used, and still needed, by the `pdk`/wasm path in `runner_pdk.rs`, whose host-exec errors carry no `ErrorKind` at all) made native "tool isn't installed" detection depend on OS-locale/libc wording rather than the type-checkable `ErrorKind` the standard library already provides for exactly this case -- a wording change or non-English locale could have silently downgraded `NotFound` to a generic `Io` error for callers that branch on `NotFound` for install-tool UX.
- **Parameterize six "one arm per kind" implementations on the value that varied between arms, instead of writing each arm out separately**
  
  No behavior change for any real caller in this set -- each factors an existing, independently-tested code path into a single shared implementation:
  
  - `callisto-manifests` gains `Requirement` (with `parse`/`render`), a real PEP 508 dependency-specifier type backed by a parse-render-parse idempotence property test. Replaces three independent from-scratch decompositions of the same marker-split/operator-split/extras-split logic in `python.rs` (`iter_dependencies`, `update_dependency_spec`, `update_optional_dependencies`). Rewriting a dependency's extras list or marker now goes through `Requirement::render`'s normalized form (e.g. `[a, b]` -> `[a,b]`, `; marker` -> `;marker`) rather than re-splicing the original raw substrings -- no existing test encoded the old whitespace-preserving behavior.
  - `callisto-manifests`'s `cargo.rs` factors `set_scalar_preserving_decor` and `set_dependency_version` out of the member-manifest (`CargoToml`) and workspace-root (`WorkspaceCargoResolver`) write paths, which independently hand-rolled the same decor-preservation dance for `[package].version` and for a dependency's bare-string/inline-table/full-table value shapes.
  - `callisto-graph`'s `config::resolve::load` factors `parse_package_config_fields` out of its `[[package]]` and `[[package-set]]` loops, which ran the identical release-trigger/tag-template/changelog/pre-major-inference/publish-to parsing sequence into an identical `PackageConfig` literal, differing only in the pattern-parser type and an error-message prefix.
  - `callisto-graph`'s `config::groups::GroupTable::validate_syntactic` and `::resolve` now loop over `[GroupKind::Fixed, GroupKind::Linked]` internally instead of writing the fixed arm and linked arm out separately. `validate_syntactic` previously had no direct unit tests; this change adds eight, covering the asymmetric cross-kind member-conflict check the single shared implementation had to reproduce exactly.
  - `callisto-changelog`'s `write::prepend` now computes `rest` once across its three header-shape branches (blank line after the H1, no blank line, no matching H1 at all) and runs the shared five-statement splice a single time, instead of each branch repeating the same splice after computing its own `rest`.
  - `callisto-moon`'s `locator.rs` factors a single `detect_single_ecosystem` helper for `declared_edges`'s from/to ecosystem lookup (previously two copies of the same if/else-if chain), and routes both `projects()`'s multi-ecosystem enumeration and `declared_edges`'s single-ecosystem lookup through `Ecosystem::CANONICAL` instead of separately hardcoded manifest-filename checks.
- **Report registry-publish propagation lag with its own error, wire up missing `[lints]` tables**
  
  `dispatch_registry`'s npm publish-succeeded-but-registry-not-yet-consistent branch used to return `GraphError::ReleaseIntentStale`, whose help text ("no release operation was authorized") is false at that call site -- an operation *was* authorized and did run; the real cause is registry propagation lag. It now returns a new `GraphError::RegistryPublishUnconfirmed { package, version }` (E157) with accurate help text. The check itself (`Ecosystem::Npm` + `PublishOutcome::Published` + registry not yet showing the version) is unchanged; only the error reported when it fires changed. Extracted the check into `require_registry_confirmation` for direct unit coverage.
  
  `callisto-fixtures` and `callisto-moon` were missing the `[lints]` table every other crate in the workspace has, so `unsafe_code = "forbid"` and the shared clippy lint set were silently never enforced on them. `callisto-fixtures` had zero latent violations. `callisto-moon`'s `tests/moon_wasm_sandbox.rs` legitimately mutates process-wide env vars (`WARPGATE_PLUGINS_DIR`, `PATH`) under `Once`/mutex-serialized, individually SAFETY-commented `unsafe` blocks for test isolation -- `forbid` cannot be locally overridden by any in-source attribute at any nesting depth, so `callisto-moon`'s `[lints.rust]` duplicates the workspace's rust-lint table with `unsafe_code` downgraded to `"deny"` (still fails by default; a local `#[allow(unsafe_code)]` permits exactly these four reviewed sites), while `[lints.clippy]` duplicates the workspace's clippy table unchanged (Cargo's `workspace = true` inherits a `[lints]` table wholesale or not at all -- no per-tool-group partial inheritance).
- Released together with the `workspace` fixed group.

## 0.6.0

- Released together with the `workspace` fixed group.

## 0.5.0

- Released together with the `workspace` fixed group.

## 0.4.1

- Released together with the `workspace` fixed group.

## 0.4.0

- **Release-pipeline / CI Action contract correctness**
  
  - **Breaking:** several `--format json` field names changed or were added (`validate`, `compose-pr-body`, `tag`, `status`, `plan-publish`) — update any scripts parsing this output.
  - Re-tagging a release that already exists no longer reports the wrong commit sha.
  - The official GitHub Action now actually opens a release PR when changesets are pending — a bug made this step unreachable before.
  - GitHub Releases are now correctly marked prerelease for PEP 440 versions too (e.g. `1.2.3a1`), not just SemVer's `-` syntax.
  - Release notes now include the real changelog section instead of nothing.
- Released together with the `workspace` fixed group.

## 0.3.0

### Minor Changes

- Release update

