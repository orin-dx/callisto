---
callisto-cli: patch
---

In pre-release mode, packages with the same name in two ecosystems (`cargo/foo`, `npm/foo`) keep separate starting versions in `pre.json` instead of sharing one.
