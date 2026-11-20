---
callisto-cli: minor
---

**The crates.io registry is configured as `[registries.crates-io]`, the same name `publish-to` uses.** Every `publish-to` value except `github-release` and `none` now names a registry, and `crates-io`, `npm` and `pypi` are predeclared.

- Declaring `[registries.crates-io]`, `[registries.npm]` or `[registries.pypi]` configures that registry; `kind` may be left out.
- A named `kind = "npm"` or `kind = "pypi"` registry cannot be a `publish-to` target yet. A named npm registry only approves a package's `publishConfig.registry`.

**Breaking:** `[registries.cratesIo]` now fails at load with E325, whose message names `[registries.crates-io]`. A predeclared registry given another `kind` (for example `[registries.npm] kind = "cargo"`) now fails at load with E334 instead of at release time. `[registries.none]`, `[registries.github-release]` and a registry name other than letters, digits, `-` and `_` fail with E325. `nuget` is no longer a `publish-to` value (E330). A release plan written by an earlier version is refused with E215; run `release plan` again.

**Action:** Rename `[registries.cratesIo]` to `[registries.crates-io]` and remove `nuget` from `publish-to`.
