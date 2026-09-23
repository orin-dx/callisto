---
callisto-graph: patch
---

**`release-trigger = "changeset"` actually skips commit inference**

A package resolved to the default `changeset` trigger no longer has commit-based severity inference run against it; only `auto` does. Previously `aggregate()` ran inference for every package regardless of the resolved trigger.
