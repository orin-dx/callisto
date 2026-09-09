---
callisto-manifests: patch
callisto-graph: patch
---

**Batch `WorkspaceCargoResolver`-routed writes to a shared root `Cargo.toml` into one open/mutate/persist cycle**

`WorkspaceCargoResolver::write_version`/`write_dependency` in `callisto-manifests` no longer call `persist` internally -- they now do pure in-memory mutation only, mirroring the `Manifest` trait's `write_version`/`update_dependency_spec`/`persist` split already used by `CargoToml`/`PackageJson`/`PyprojectToml`. `persist` is promoted to a public, explicitly-called method. Every existing call site (the inherited-dependency delegation inside `CargoToml::update_dependency_spec`, and both of `apply_version_plan`'s per-write fallback loops) now calls `persist` explicitly afterward -- no behavior change there.

`callisto-graph`'s `classify_manifest_writes`/`apply_version_plan` (`apply.rs`) now route root-manifest writes (`VersionWriteTarget::CargoWorkspacePackage`/`DepWriteTarget::CargoWorkspaceDependency`) into a new `resolver_batched` group when the same physical root manifest receives no competing write through the `Manifest` trait. A release that bumps the workspace version plus several workspace-inherited dependencies now opens the root `Cargo.toml` once, applies every mutation in memory, and persists exactly once -- down from one read+parse+atomic-write per write. The pre-existing mixed-routing exclusion (a root manifest written through *both* the `Manifest` trait and `WorkspaceCargoResolver` -- the data-loss/ordering hazard `classify_manifest_writes` already guarded against) is unchanged: those paths still get no batching on either side, processed individually and in the same order as before.

New `resolver_load_call_count`/`resolver_persist_call_count` test-observability counters (kept separate from `open_call_count`/`persist_call_count`, since `OpenContext::for_workspace_root` calls `WorkspaceCargoResolver::load` unconditionally whenever a workspace root `Cargo.toml` exists, which would otherwise pollute those counters workspace-wide) prove the new batched path opens and persists the shared root manifest exactly once per `apply_version_plan` call, regardless of how many bumps/rewrites target it, while the mixed-routing case continues to persist each write individually.
