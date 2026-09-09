---
callisto-manifests: patch
callisto-graph: patch
---

`ManifestWalkResolver::build`'s napi-platform-package detection did its own raw `fs::read` + `serde_json::from_slice` on every discovered npm `package.json`, entirely outside `callisto-graph`'s `manifest_cache` -- the same mechanism that exists precisely so a manifest path is read and parsed at most once per run. The exact same file was then opened again moments later, through the cache, for `publish_targets`/`iter_dependencies`.

`Manifest` gains an `npm_role() -> Option<NpmRole>` method (default `None`), overridden by `PackageJson` to derive the napi platform/arch/abi role from the document already parsed at `open()` time. `ManifestWalkResolver::build` now calls `.npm_role()` on the manifest handle it gets from `open_cached`, so each npm manifest is read from disk once, not twice.
