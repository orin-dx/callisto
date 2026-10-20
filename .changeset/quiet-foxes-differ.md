---
callisto-cli: patch
---

A package that took a name from a renamed or removed package in another ecosystem no longer shows that package's tag as its own last release, so it is no longer skipped as already released. `status` says which tag was rejected and points to `previous-tag-templates`.
