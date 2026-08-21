---
callisto-graph: minor
---

# Fixed groups converge on a single, shared target version

- Siblings in a `[[fixed-group]]` now bump to one shared target instead of drifting to independent versions when multiple changesets in the group carry different severities.
- `pre_mutation_checks` (divergence detection, napi-drift detection) is now actually wired into `plan_version`'s orchestration; a diverged group aborts with a clear error instead of silently producing inconsistent versions.
- `GraphError::ConflictingGroupMembership` is now actually detected: two differently-spelled group-member strings that resolve to the same package across two groups are caught instead of silently accepted.
- The workspace itself is now declared as a `[[fixed-group]]` in `callisto.toml`, and a regression test pins that two independent changesets in one group converge to a single bump (e.g. two minor changesets converge to one minor bump, not a compounded double bump).
