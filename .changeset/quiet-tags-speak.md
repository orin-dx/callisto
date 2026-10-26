---
callisto-cli: patch
---

`release` now warns when a package's latest tag exists locally but not on `origin`, including in `--dry-run`. This exposes an unfinished tag push instead of reporting `Nothing to release.`
