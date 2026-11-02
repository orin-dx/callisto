---
callisto-cli: patch
---

A private root `package.json` that starts with a byte-order mark is no longer listed as an npm package, which failed `status` with E013.
