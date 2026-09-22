---
callisto-cli: minor
---

**Release artifacts are declared, not compiled in**

`[release] artifact-targets` is replaced by `[[release.artifact]]` entries, each naming the `package` that builds it, an opaque `target`, and its `asset-name`. Previously the four supported targets and their `callisto-*` asset names were compiled into Callisto and the configured list was validated then ignored, so no other workspace could use `[release]`. Any targets and names are now accepted; an asset-name must be unique and a single safe path component.

An artifact built by a package other than the product (for example a plugin shipped on the product's GitHub Release) is now recorded under that package in the release intent. Selecting one member of a `[[fixed-group]]` with `--package` now keeps its lockstep siblings in the release, so a sibling's artifact no longer fails with an unexplained "not authorized" error and no bumped sibling is left unpublished. That error now names the asset, its package and the fix.

Migrate each target to a block:

```toml
[[release.artifact]]
package = "cargo/my-cli"
target = "aarch64-apple-darwin"
asset-name = "my-cli-aarch64-apple-darwin.tar.gz"
```
