---
callisto-graph: patch
callisto-fixtures: patch
callisto-moon: patch
---

**Report registry-publish propagation lag with its own error, wire up missing `[lints]` tables**

`dispatch_registry`'s npm publish-succeeded-but-registry-not-yet-consistent branch used to return `GraphError::ReleaseIntentStale`, whose help text ("no release operation was authorized") is false at that call site -- an operation *was* authorized and did run; the real cause is registry propagation lag. It now returns a new `GraphError::RegistryPublishUnconfirmed { package, version }` (E157) with accurate help text. The check itself (`Ecosystem::Npm` + `PublishOutcome::Published` + registry not yet showing the version) is unchanged; only the error reported when it fires changed. Extracted the check into `require_registry_confirmation` for direct unit coverage.

`callisto-fixtures` and `callisto-moon` were missing the `[lints]` table every other crate in the workspace has, so `unsafe_code = "forbid"` and the shared clippy lint set were silently never enforced on them. `callisto-fixtures` had zero latent violations. `callisto-moon`'s `tests/moon_wasm_sandbox.rs` legitimately mutates process-wide env vars (`WARPGATE_PLUGINS_DIR`, `PATH`) under `Once`/mutex-serialized, individually SAFETY-commented `unsafe` blocks for test isolation -- `forbid` cannot be locally overridden by any in-source attribute at any nesting depth, so `callisto-moon`'s `[lints.rust]` duplicates the workspace's rust-lint table with `unsafe_code` downgraded to `"deny"` (still fails by default; a local `#[allow(unsafe_code)]` permits exactly these four reviewed sites), while `[lints.clippy]` duplicates the workspace's clippy table unchanged (Cargo's `workspace = true` inherits a `[lints]` table wholesale or not at all -- no per-tool-group partial inheritance).
