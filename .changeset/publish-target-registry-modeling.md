---
callisto-graph: minor
callisto-model: minor
callisto-manifests: patch
---

# npm publish access modeled as a 3-state enum

## Breaking Changes

- **`PublishTarget::Npm.restricted: bool` → `access: Option<NpmAccess>`.** npm's `publishConfig.access` has three real states (unset, public, restricted); a bool couldn't hold them. An explicit `"public"` on an unscoped package was previously dropped silently.

## Fixes

- npm dist-tag, PyPI index, and Cargo registry selection are now threaded through the publish boundary consistently.
