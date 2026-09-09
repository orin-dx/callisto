---
callisto-vcs: patch
---

**Bump `gix` 0.86 -> 0.87 to drop the fully-yanked `bisync` transitive dependency**

`gix` 0.86's dependency chain (`gix` -> `gix-protocol` 0.64 -> `bisync` ^0.3.0) pulled in `bisync`, every published version of which (0.1.0 through 0.3.1) is now yanked from crates.io -- there is no non-yanked version to pin instead. `gix-protocol` 0.65.0 already replaced `bisync` with `async-trait`/`futures-lite` for its async-generic code generation, so bumping `gix` to 0.87 (which resolves to `gix-protocol` 0.65.1) removes `bisync` from the dependency tree entirely (confirmed: zero occurrences in `Cargo.lock`).

This does not itself fix `just check-api` (`cargo semver-checks check-release`): that command always runs `cargo update` on whatever baseline it builds (the last version published to crates.io, or a specified git revision), and every currently-published version of `callisto-vcs`/`callisto-cli` still requires the old, now-unresolvable `gix` 0.86 chain. `cargo semver-checks` has no flag to skip that baseline `cargo update`, and Cargo itself has no mechanism to force-accept a yanked version during fresh dependency resolution. This is a one-time transition gap: the next version published with this fix becomes the new baseline for all future `check-api` runs, at which point the tool resolves cleanly again since the fixed baseline never touches `bisync`.
