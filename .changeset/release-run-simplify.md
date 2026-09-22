---
callisto-cli: minor
---

**Rerunning a failed release completes it**

`release execute` now adopts every effect that already landed, including a published registry version (one warning line each), and builds the receipt from in-memory execution state, so "Re-run failed jobs" finishes a partial release instead of failing with E174. Execution state is no longer persisted, and E173 and E174 are gone.

Breaking:
- `release execute --recovery` and `--state` are removed.
- `release reconcile` and `callisto schema --type release-state` are removed.
- The receipt's run envelope drops `kind` and is envelope schema v2.
