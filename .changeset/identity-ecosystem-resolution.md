---
callisto-graph: minor
---

**Better cross-ecosystem package discovery**

- Packages excluded from your Cargo/npm/pnpm workspace (fuzz targets, scratch examples) no longer show up as release-managed.
- A Rust crate and its same-named Python binding in one workspace no longer trigger a duplicate-package error.
- `cargo:name`/`npm:name` prefixes in group config now work as documented.
- A package name is no longer silently matched to the wrong ecosystem when the intended match is missing.
