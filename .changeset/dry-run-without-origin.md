---
callisto-cli: patch
---

`callisto release --dry-run` now previews a release that builds artifacts when there is no `origin`, instead of failing with E170. A real release still needs one.
