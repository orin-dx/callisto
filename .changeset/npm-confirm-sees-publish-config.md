---
callisto-cli: patch
---

When the release action is given `NPM_TOKEN`, the `npm view` that confirms a publish now uses the same temporary npm config as the publish, so a restricted scoped package is found instead of timing out with E157.
