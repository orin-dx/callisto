---
callisto-graph: minor
---

# Better cross-ecosystem package discovery

- Cargo/npm/pnpm workspace membership is now honored during project discovery — fuzz targets and scratch examples excluded from a workspace stop showing up as release-managed packages.
- Same-named packages in different ecosystems (a Rust crate and its Python binding, say) resolve correctly instead of erroring with a duplicate-package conflict.
- `cargo:foo`/`npm:foo` ecosystem-prefixed group-member names now work.
- A silent cross-ecosystem fallback in native-identity resolution (matching a same-named package in the wrong ecosystem when the intended lookup missed) has been removed.
