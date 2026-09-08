---
callisto-model: minor
callisto-manifests: minor
callisto-graph: patch
---

**Add a shared canonical-manifest-identity reader; stop hand-parsing Cargo.toml/package.json/pyproject.toml independently in six places**

`callisto-model` gains `Ecosystem::CANONICAL` (`[Cargo, Npm, Pypi]`) and `Ecosystem::canonical_manifest_format()`, the single enumeration of "which ecosystems have a canonical identity manifest, and which format is it" -- replacing hand-written `if root.join("Cargo.toml").exists() { ... } else if ...` chains.

`callisto-manifests` gains `read_identity(format, source, path) -> Result<ManifestIdentity, ManifestError>`, a pure, I/O-free reader for callers that already hold a manifest's content as a string (a `git show` blob, a directory walker's pre-read buffer) instead of a path `Manifest::open` can read from disk. `ManifestIdentity { name, version }` carries `version` as a new `VersionSource` enum (`Literal(String)` vs `InheritedFromWorkspace`) rather than collapsing Cargo's `version.workspace = true` into `None`. Also adds `read_napi_targets(path, &Value) -> Result<Option<Vec<String>>, ManifestError>`, the one shared parser for `napi.targets`.

Six call sites in `callisto-graph`/`callisto-moon` now route through these instead of re-implementing the same extraction: `IdentityResolver::resolve`, `IgnoreWalkLocator::projects`, `MoonProjectLocator`'s ecosystem detection, workspace-membership manifest-filename lookups, `NapiTargetsIndex::load`, and `matrix::read_napi_targets`.

Two real behavior changes fall out of this:

- **`IgnoreWalkLocator::projects()` now discovers Flit-based Python packages.** Its own pyproject.toml parsing previously only checked `project.name` and `tool.poetry.name`, missing the `tool.flit.metadata.module` fallback the shared extractor already had -- a Flit package was silently undiscovered before this change.
- **`NapiTargetsIndex::load` and `matrix::read_napi_targets` now share one parser with their policy difference visible at the call site**: both still disagree on how to handle a malformed `napi.targets` (`NapiTargetsIndex::load` stays lenient via `.ok()`, `matrix::read_napi_targets` stays strict by propagating the error), but a `napi.targets` array containing a non-string entry now causes `NapiTargetsIndex::load` to drop the whole array instead of silently filtering out just the bad entry -- a narrower form of the same lenient policy, no longer a second independent implementation.

`GraphError`'s `manifest_version_at` (release-commit verification against a historical git blob) also now goes through `read_identity` instead of its own inline `toml_edit`/`serde_json` parsing.
