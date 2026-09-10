---
callisto-graph: patch
---

**`release.rs` reports its real failure cause instead of a generic stale-intent error (final PR of the migration)**

PR4 of the 4-PR `SPEC-ARCH-RELEASE-ERROR-TAXONOMY` migration (PR1: #79, PR2: #80; PR3 migrates `release_decision.rs` separately). All 44 `StaleReason::legacy_unclassified()` sites in `commands/release.rs` -- registry publish dispatch, tag/forge dispatch, and release-intent/operation derivation -- now report a specific `GraphError` cause instead of the generic "no release operation was authorized" message:

- Registry classification failures (`classify_cargo_output`/`classify_npm_publish_output`/`classify_twine_output`) now surface `GraphError::Registry { package, source }` (E163), carrying the real `RegistryError::{AuthFailed,RateLimited,Network,Other}`.
- Non-zero subprocess exits and malformed output for `git`/`gh`/`npm` (push, tag observation, `for-each-ref`, `gh release create`, `gh api`, `npm view`, the pypi build step) now surface `GraphError::ReleaseCommand { program, args, failure }` (E164) with `CommandFailure::NonZeroExit`/`MalformedOutput`.
- Remote-object conflicts observed after an authorized effect (a tag at a different commit, a tag not observed after push, a forge release that already differs or wasn't observed after creation, an unexpected forge API status) now surface `GraphError::ReleaseRemoteConflict { conflict }` (E167), using all five `RemoteConflict` variants.
- Unsupported ecosystem/source-identity/publish-target combinations now surface `GraphError::UnsupportedRelease { feature }` (E168).
- A registry publish target that would collapse two operations into one now surfaces `GraphError::ReleaseSelectionInvalid { package, reason: DuplicateRegistryTarget }` (E169).
- Unmet preconditions (a non-detached HEAD, a canonical root that doesn't match the workspace root, no git remote prepared, no GitHub remote configured) now surface `GraphError::ReleasePreconditionUnmet { requirement }` (E170).
- Internal invariants (an unknown prepared-operation id, a `PreparedOperation`/pypi-argv shape mismatch, a for-each-ref line with no annotation field, a package with no canonical manifest, an operation-order cycle, a publish target with no registry key) now surface `GraphError::ReleaseInvariant { detail }` (E171).
- Model-constructor failures (`ReleaseIntentV1::new`, `ReleaseOperation::{registry_publish,new,tag,forge_release}`, `RegistryBindingId::new`, `ReleaseInputSnapshotV1::new`) now propagate via `?` through the existing `#[from]` conversions (E158/E159/E161) instead of being discarded.
- Three pre-existing, StaleReason-unrelated `map_err(|_error| ...)` discards in `canonical_registry_binding`/`canonical_git_remote` (URL and GitHub-repository parse failures) now classify the real error into a specific static reason instead of a single generic one.

`crates/callisto-graph/tests/architecture.rs`: `legacy_unclassified_ratchet`'s `commands/release.rs` count is lowered from 44 to 0, and `commands/release.rs` is removed from `MAP_ERR_IGNORE_ALLOWLIST` entirely (zero `map_err(|_ident| ...)` discards remain in the file). The `#[deprecated]` `StaleReason::legacy_unclassified()` constructor itself stays defined -- `commands/release_decision.rs` (PR3, landing separately) still uses it -- and will be deleted once that file also reaches zero.

No behavior change beyond error messages/codes becoming specific to their real cause.
