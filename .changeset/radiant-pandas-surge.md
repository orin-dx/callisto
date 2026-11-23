---
callisto-cli: patch
---

**A release is no longer refused for a dependency in a manifest it does not publish.** Releasing a package whose npm manifest depends on a package you have not released, while the release publishes only the crate, now proceeds.
- A dependency is required only when the dependent's own manifest declares it and the dependency publishes to that registry.
