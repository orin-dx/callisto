---
callisto-graph: patch
---

**Fix: a dev-only publish cycle no longer un-orders unrelated dev-dependencies**

`publish_order` previously dropped dev-dependency ordering for the entire publish batch the moment any one legitimate dev-only cycle existed (e.g. two crates mutually dev-depending on each other for cross-integration tests). The exclusion is now scoped to just the cyclic pair — every other dev-dependency ordering in the same batch is still honored.
