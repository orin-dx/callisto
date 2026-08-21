---
callisto-graph: minor
---

**Platform-manifest and `optionalDependencies` write planning**

- Fixed groups with platform-specific packages (napi/maturin) now get their platform manifests and `optionalDependencies` updated automatically during a version bump — previously nothing happened.
