---
callisto-manifests: minor
callisto-graph: patch
callisto-changelog: patch
callisto-moon: patch
---

**Parameterize six "one arm per kind" implementations on the value that varied between arms, instead of writing each arm out separately**

No behavior change for any real caller in this set -- each factors an existing, independently-tested code path into a single shared implementation:

- `callisto-manifests` gains `Requirement` (with `parse`/`render`), a real PEP 508 dependency-specifier type backed by a parse-render-parse idempotence property test. Replaces three independent from-scratch decompositions of the same marker-split/operator-split/extras-split logic in `python.rs` (`iter_dependencies`, `update_dependency_spec`, `update_optional_dependencies`). Rewriting a dependency's extras list or marker now goes through `Requirement::render`'s normalized form (e.g. `[a, b]` -> `[a,b]`, `; marker` -> `;marker`) rather than re-splicing the original raw substrings -- no existing test encoded the old whitespace-preserving behavior.
- `callisto-manifests`'s `cargo.rs` factors `set_scalar_preserving_decor` and `set_dependency_version` out of the member-manifest (`CargoToml`) and workspace-root (`WorkspaceCargoResolver`) write paths, which independently hand-rolled the same decor-preservation dance for `[package].version` and for a dependency's bare-string/inline-table/full-table value shapes.
- `callisto-graph`'s `config::resolve::load` factors `parse_package_config_fields` out of its `[[package]]` and `[[package-set]]` loops, which ran the identical release-trigger/tag-template/changelog/pre-major-inference/publish-to parsing sequence into an identical `PackageConfig` literal, differing only in the pattern-parser type and an error-message prefix.
- `callisto-graph`'s `config::groups::GroupTable::validate_syntactic` and `::resolve` now loop over `[GroupKind::Fixed, GroupKind::Linked]` internally instead of writing the fixed arm and linked arm out separately. `validate_syntactic` previously had no direct unit tests; this change adds eight, covering the asymmetric cross-kind member-conflict check the single shared implementation had to reproduce exactly.
- `callisto-changelog`'s `write::prepend` now computes `rest` once across its three header-shape branches (blank line after the H1, no blank line, no matching H1 at all) and runs the shared five-statement splice a single time, instead of each branch repeating the same splice after computing its own `rest`.
- `callisto-moon`'s `locator.rs` factors a single `detect_single_ecosystem` helper for `declared_edges`'s from/to ecosystem lookup (previously two copies of the same if/else-if chain), and routes both `projects()`'s multi-ecosystem enumeration and `declared_edges`'s single-ecosystem lookup through `Ecosystem::CANONICAL` instead of separately hardcoded manifest-filename checks.
