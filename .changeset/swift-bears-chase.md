---
callisto-cli: minor
---

**Each registry has one name, used in both `[registries]` and `publish-to`.** You configure crates.io as `[registries.crates-io]`, the same name you publish to, and a mistyped registry kind is caught when the config loads instead of during a release.

- **How to adopt:** `crates-io`, `npm` and `pypi` always exist. Add `[registries.crates-io]`, `[registries.npm]` or `[registries.pypi]` only to change a setting such as `confirm-timeout`; `kind` can be left out.
- **Limits:** a custom registry with `kind = "npm"` or `kind = "pypi"` cannot be named in `publish-to` yet.
- **Limits:** a custom npm registry's `url` only approves a package's `publishConfig.registry`.

**Breaking:** `[registries.cratesIo]` used to configure crates.io. It now fails when the config loads with E325, and the message names the new spelling. Rename it to `[registries.crates-io]`.

**Breaking:** `crates-io`, `npm` or `pypi` given another `kind` (for example `[registries.npm] kind = "cargo"`) used to load and then fail during a release. It now fails when the config loads with E334. Remove the `kind`, or give a custom registry another name.

**Breaking:** A registry named `none` or `github-release`, or with characters other than letters, digits, `-` and `_`, used to load. It now fails with E325. Rename the registry.

**Breaking:** `publish-to = ["nuget"]` used to load and then fail during a release. It now fails when the config loads with E330. Remove `nuget` from `publish-to`.

**Breaking:** A release plan saved by an earlier version used to be read. It is now refused with E215. Run `callisto release plan` again.

**Action:** Rename `[registries.cratesIo]` to `[registries.crates-io]` and remove `nuget` from `publish-to` before upgrading.
