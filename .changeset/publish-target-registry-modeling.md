---
callisto-graph: minor
callisto-model: minor
callisto-manifests: patch
---

**npm publish access modeled as a 3-state enum**

- **Breaking:** `plan-publish`'s npm target now reports `access` (`"public"`/`"restricted"`/unset) instead of a `restricted: bool` — update anything parsing that JSON field.
- An unscoped npm package with `publishConfig.access: "public"` in its `package.json` is no longer silently dropped during publish planning.
