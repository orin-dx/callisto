---
callisto-cli: patch
---

`release` and `release --dry-run` on a workspace with many packages are about twice as fast: registry lookups and per-package history checks now run side by side, and the workspace is loaded once instead of three times.
