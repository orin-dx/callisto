---
callisto-model: patch
callisto-graph: patch
---

What each `PublishTarget` variant means -- whether it's dispatchable, how it contributes to a package's semantic fingerprint, and whether it carries an explicit registry override -- was independently re-encoded in three separate match statements (`callisto-graph`'s `target_fingerprint`, `prepared_registry_binding`, and `publish.rs`'s per-target dispatch loop), kept in sync only by convention with `callisto-model`'s existing `PublishTarget`/`Ecosystem` capability layer.

Extended that existing layer instead of adding a second one: `PublishTarget` gains `registry_override()` (the `Npm.registry`/`Pypi.index`/`NuGet.source` payload field, unified) and `is_implemented()`. `PublishTarget` and `Ecosystem` are not 1:1 -- `GitHubRelease` is a VCS release action with no backing package `Ecosystem`, and `None` is the "not configured to publish" sentinel -- so `is_implemented()` special-cases both (`GitHubRelease` is always unimplemented, `None` is vacuously implemented) and otherwise defers to `Ecosystem::is_implemented()`. `target_fingerprint` now reuses the existing `config_str()` instead of re-hardcoding its own kind strings; `prepared_registry_binding` now calls `registry_override()` instead of its own separate match; `publish.rs`'s dispatch loop now decides its `PublishTargetNotImplemented` diagnostic via `is_implemented()` instead of independently naming `NuGet`/`GitHubRelease`. Pure refactor -- no behavior change for any existing test case.
