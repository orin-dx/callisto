---
callisto-cli: patch
---

**`publish-to` overrides keep `publishConfig`**

A `publish-to = ["npm"]` override no longer drops `publishConfig.access` and `registry` from `package.json`, which made scoped packages publish as restricted. Explicit config still wins.
