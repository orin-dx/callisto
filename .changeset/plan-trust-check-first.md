---
callisto-cli: patch
---

`callisto release plan --from-release-commit` now checks the repository first, so a shallow clone fails with E266 and a SHA-256 repository with E265, not a lower-level read error.
