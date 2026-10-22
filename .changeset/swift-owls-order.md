---
callisto-cli: patch
---

A package published to several registries now orders each distribution on its own. A dependent follows only the distribution it depends on: a Cargo dependent follows the crate and an npm dependent follows the npm package, so a failed publish in one ecosystem no longer holds back dependents in the other. A product's GitHub release is no longer made public before the registry publishes of a plugin package whose asset it carries have finished. `callisto release --package` on one distribution, such as `npm/dual`, releases every distribution of that package, so its shared tag and GitHub release no longer follow a single publish. An intent planned by an older version is rejected as stale and needs `release plan` again.
