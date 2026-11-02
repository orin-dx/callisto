---
callisto-cli: patch
---

`release-pr commit-plan` now fails if a Git filter such as CRLF conversion changes a staged file's bytes, and rejects a staged symlink or submodule.
