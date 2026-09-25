---
callisto-cli: patch
---

`version` now refuses to run again after a previous run wrote its changes but was never committed, and validates `--strict` escalation before writing anything instead of writing first and failing afterward.
