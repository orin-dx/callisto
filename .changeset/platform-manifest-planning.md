---
callisto-graph: minor
---

# Platform-manifest and `optionalDependencies` write planning

`plan_version` now actually computes platform-manifest writes and `optionalDependencies` updates for a bumped owner's platform siblings in a Fixed group (`VersionPlan.platform_writes`/`optional_dep_updates`). These fields were previously always empty, so `apply_version_plan`'s consumption side had nothing to act on.
