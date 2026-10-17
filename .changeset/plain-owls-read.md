---
callisto-cli: patch
---

Yarn classic's `"workspaces": { "packages": [...] }` form is now read for workspace membership, and a root Cargo workspace that cannot be loaded for version inheritance is reported instead of silently dropping the packages that inherit from it.
