---
callisto-cli: minor
---

`callisto matrix` no longer collides a Cargo crate and an npm package that share a bare name (the common napi split layout).

`--package` now accepts an ecosystem-qualified id (`cargo/foo`, `npm/foo`); a bare name resolves only when it names exactly one package, otherwise it errors listing the qualified candidates. A `[[release.artifact]]` binary now binds only to its own cargo package, never to a same-named package in another ecosystem.

Breaking:
- `platformTargets`/`runtimeVersions` map keys are each package's ecosystem-qualified id once a same-named package in another ecosystem exists, not always the bare name. Consumers indexing by bare name must switch to the qualified id (or keep working unchanged for any package whose name is unique in the workspace).
