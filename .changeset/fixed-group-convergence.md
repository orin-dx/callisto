---
callisto-graph: minor
---

**Fixed groups converge on a single, shared target version**

- Packages in a `[[fixed-group]]` now bump to the same version together, instead of drifting apart when they carry changesets of different severity.
- A group that would end up in an inconsistent state now aborts with a clear error instead of silently shipping a broken version.
- Two differently-spelled group-member entries that actually point to the same package are now caught as a config error instead of silently accepted.
