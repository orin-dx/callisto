---
callisto-cli: patch
---

When a release takes a Python dependency out of its `~=` range, the rewritten range keeps the precision it was written with: `~=1.2` becomes `~=2.0`, not `~=2.0.0`, which would have excluded every later 2.x release.
