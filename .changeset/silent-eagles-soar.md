---
callisto-cli: patch
---

`callisto version --emit-decision` no longer fails when a dependent is bumped because of a package published to both crates.io and npm. The cascade reason names the distribution the dependent depends on.
