---
callisto-graph: patch
callisto-cli: patch
---

**Add `--skip-publish-precheck` to skip the redundant already-published registry check**

`callisto publish` previously called the registry's `is_published` check before every publish attempt, even though a fresh publish always returns false there and a conflicting publish is already correctly classified as `AlreadyPublished` from the publish call itself. Pass `--skip-publish-precheck` to skip that extra round-trip; the default behavior is unchanged.
