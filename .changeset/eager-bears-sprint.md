---
callisto-cli: patch
---

**Fixed groups no longer leak provisional versions into dependency ranges.** When a fixed group aligned its members to a lower version than a member's own bump, a dependent's requirement could be rewritten to the provisional version, widening a range, or rejected (E319) for a version that was never released.

- Rewrites and E319 now follow the group's final aligned versions, for Cargo, npm and Python.
- If a run aborted with E319 naming a version your fixed group never released, re-run it.
