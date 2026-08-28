---
callisto-graph: patch
---

**Fix: skip an npm main package when its platform dependency fails to publish**

Previously an npm main package would still publish even when a declared platform dependency (`optionalDependencies`) failed in the same run, shipping a version that referenced an unpublished package. The dependency-failure check is scoped to the npm ecosystem, so a same-named Cargo crate failure can no longer false-positive-match an unrelated npm platform dependency.
