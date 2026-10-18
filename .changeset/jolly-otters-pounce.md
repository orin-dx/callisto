---
callisto-cli: minor
---

A Cargo workspace below the repository root, such as `native/` or `rust/`, is now supported. Its members inherit `version.workspace = true` and `{ workspace = true }` dependencies from it instead of failing with E313, `callisto version` refreshes and stages the `Cargo.lock` beside it, and a release commit is verified against it. Its own `members` and `exclude` decide which crates in it are packages.
