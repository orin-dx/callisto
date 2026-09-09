---
callisto-manifests: patch
---

`cargo.rs`'s `set_scalar_preserving_decor` (look up an existing scalar, clone its decor, build the new value, reapply decor if present, else insert fresh) and `python.rs`'s local `set_with_decor` closure inside `PyprojectToml::write_version` were the same CST-editing algorithm, differing only in operating on `toml_edit::Table::insert` versus `toml_edit::Item` indexing.

Lifted into a single shared `common::set_scalar_preserving_decor(table: &mut dyn toml_edit::TableLike, key, new_value)`, since `toml_edit::TableLike` is implemented by both `Table` (Cargo's `[package]`/`[workspace.package]` tables, which coerce to `&mut dyn TableLike` directly) and reachable from an `Item` via `Item::as_table_like_mut()` (pyproject.toml's `[project]`/`[tool.poetry]`/`[tool.flit.metadata]`, each held as an `Item`). Both `cargo::CargoToml::write_version`, `cargo::WorkspaceCargoResolver::write_version`, and `python::PyprojectToml::write_version` now call the shared implementation; the two local copies are deleted. No behavior change -- decor-preservation tests for both ecosystems pass unchanged.
