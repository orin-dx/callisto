---
callisto-cli: patch
---

Generated release workflows pass the detected default branch to the release-PR action, and `init` now resolves it from a live remote query or the checked-out branch when `origin/HEAD` isn't set locally, instead of silently defaulting to `main`.
