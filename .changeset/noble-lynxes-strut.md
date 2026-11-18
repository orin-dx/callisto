---
callisto-cli: patch
---

**A package published to both crates.io and npm no longer waits on the other registry's dependencies.** A Cargo-only dependency now orders only the package's crate, and an npm-only dependency only its npm package, so one registry's publish is not held back by the other's.
