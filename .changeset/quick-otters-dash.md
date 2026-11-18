---
callisto-cli: minor
---

**Peer dependents now get a patch, not a major.** An out-of-range peer-dependency bump releases the dependent with `cascade.bump-severity` (patch by default), matching Changesets v3.

**Breaking:** `cascade.peer-escalation` now defaults to `false`. Before, an out-of-range peer bump from a minor or major source released the dependent as a major. Now it gets `bump-severity`. To keep the old behavior, set `peer-escalation = true` under `[cascade]`, or add an explicit major changeset for the dependent.
