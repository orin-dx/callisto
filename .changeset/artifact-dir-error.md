---
callisto-cli: patch
---

`release execute` given a manifest that seals Python files but no `--artifact-dir` now asks for the directory (E233) instead of telling you to drop both flags (E232).
