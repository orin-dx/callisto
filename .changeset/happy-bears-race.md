---
callisto-cli: patch
---

**Releases that publish only to crates.io no longer fail with a false dependency cycle (E104).** A package with both a `Cargo.toml` and a `package.json` now releases its crates even when the npm manifests, which the release does not publish, depend on each other.

- A cycle between packages the release does publish still fails with E104.
