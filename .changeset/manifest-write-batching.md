---
callisto-graph: minor
callisto-manifests: minor
callisto-model: minor
---

**Manifest writes are batched and more reliable**

- **Breaking (custom manifest integrations only):** implementing callisto's `Manifest` trait now requires a `persist()` method to actually write your changes.
- Re-running a version bump is now safe and won't risk losing a write.
- Multiple changes to the same manifest file in one run are now applied together instead of risking one overwriting another.
