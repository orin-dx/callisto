---
callisto-cli: patch
---

`version` checks `--strict` before writing anything, and refuses with E121 while an earlier run's changes are uncommitted and its changesets are still pending.
