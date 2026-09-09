---
callisto-manifests: patch
callisto-graph: patch
---

**Add `OpenContext::for_workspace_root`; stop reimplementing its construction independently in four places, and stop re-reading a platform owner's manifest once per platform target**

`callisto-manifests` gains `OpenContext::for_workspace_root(root)`, the single reusable constructor for the "check `Cargo.toml` exists, load `WorkspaceCargoResolver` inheritance, detect npm workspace kind" sequence. `callisto-graph`'s `ManifestWalkResolver::build`, `Workspace::base_versions`, `apply_version_plan`, and `plan_version`'s manifest-write path each independently reimplemented this same ~15-line block; all four now call the shared constructor. No behavior change -- the resolution logic is byte-for-byte the same, just no longer copy-pasted.

`plan_version`'s platform-target loop also stopped re-opening (re-reading and re-parsing from disk) a fixed-group owner's canonical manifest once per platform-manifest sibling purely to check for an existing matching optional dependency -- napi/native cross-compile owners typically have 4-8+ platform targets. That read now routes through the workspace's existing read-only `manifest_cache`, so the owner's manifest is opened at most once per run regardless of how many platform siblings it has.
