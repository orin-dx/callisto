---
callisto-cli: patch
---

`callisto release --package` on one distribution of a package that publishes to several registries, such as `npm/dual` for a Cargo and npm package, now releases every distribution of that package, so its shared tag and GitHub release no longer follow a single publish.
