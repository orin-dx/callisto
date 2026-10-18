---
callisto-cli: patch
---

The interrupted-run check now finds a staged version bump in the Cargo workspace a package named at `HEAD` even when its working-tree manifest already names a different `package.workspace`.
