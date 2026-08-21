---
callisto-graph: minor
callisto-manifests: minor
callisto-model: minor
---

# Manifest writes are batched and more reliable

## Breaking Changes

- **`Manifest::persist` is now a required trait method.** `write_version`, `update_dependency_spec`, and `update_optional_dependencies` only mutate in memory now — call `persist()` to write. Custom `Manifest` implementors must add it.

## Fixes

- `apply_version_plan` is now idempotent, refreshes the npm lockfile, and persists writes correctly — previously risked a lost write.
- Manifest writes to the same physical file are now batched into one open/mutate/persist cycle, processed strictly in order, closing a stale-handle data-loss bug where two separate write mechanisms touching the same file could clobber each other.
