---
callisto-cli: patch
---

**`publish-to = ["npm"]` no longer drops `publishConfig.access`/`registry`**

A `[[package]]`/`[[package-set]]` `publish-to = ["npm"]` override replaced the whole manifest-derived npm target, silently dropping `publishConfig.access` and `publishConfig.registry` read from `package.json`. A scoped package with no override-level access then published with npm's `restricted` default instead of the intended `public`, and the same dropped access was reused for any attached platform packages. The override now fills only the fields it doesn't itself set from the manifest; explicit config still wins.
