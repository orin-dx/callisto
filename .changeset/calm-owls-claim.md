---
callisto-cli: patch
---

A package that takes a name from a renamed or removed package, including one in another ecosystem, no longer takes over that package's pre-release baseline or release tags, so it is no longer skipped as already released. `status` says which tag was rejected and points to `previous-tag-templates`.
