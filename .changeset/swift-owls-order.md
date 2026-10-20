---
callisto-cli: patch
---

A package that depends on one distribution of a package published to several registries no longer waits for its other distributions: a Cargo dependent follows the crate and an npm dependent follows the npm package, so a failed publish in one ecosystem no longer holds back dependents in the other. An intent planned by an older version is rejected as stale and needs `release plan` again.
