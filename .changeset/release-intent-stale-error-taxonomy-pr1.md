---
callisto-graph: minor
---

**`GraphError::ReleaseIntentStale` (E124) becomes a struct variant carrying a real reason**

PR1 of a 4-PR migration (SPEC-ARCH-RELEASE-ERROR-TAXONOMY) fixing a confirmed architectural defect: `ReleaseIntentStale` was a fieldless unit variant constructed at ~95 sites across `commands/{release.rs,release_decision.rs,release_execution.rs}`, of which only 6 were genuine staleness -- the rest silently discarded an already-typed failure cause (a model error, a subprocess exit status, a merged-commit verification mismatch, a remote conflict, and more) behind the same generic, sometimes misleading "no release operation was authorized" message.

- `ReleaseIntentStale` is now `{ reason: StaleReason }`; its Display message appends the reason, and its diagnostic code/help text are unchanged. This is semver-breaking for any external consumer pattern-matching the old unit variant (none exist inside this workspace).
- `StaleReason` (`callisto-graph::commands::release`) is nameable everywhere but constructible with a real reason only from within `release.rs` itself -- `trust_evidence_changed`, `source_identity_changed`, `git_remote_changed`, and `intent_differs_from_fresh_derivation` cover the module's 6 genuine fresh-re-observation sites. Every other prior `ReleaseIntentStale` site (44 in `release.rs`, 42 in `release_decision.rs`, 2 in `release_execution.rs`) now uses a temporary, `#[deprecated]`, `pub(crate)` `StaleReason::legacy_unclassified()` migration ratchet, pinned exactly by a new architecture test and lowered to zero across PR2-PR4 as each site is given its real cause.
- Twelve new `GraphError` variants (E158-E171) give a typed home to every previously-discarded cause -- model construction errors, registry failures, subprocess command failures, merged-commit verification mismatches, remote conflicts, unsupported feature combinations, invalid selections, unmet preconditions, and internal invariants -- ready for PR2-PR4 to wire into their real call sites.
- New architecture tests in `crates/callisto-graph/tests/architecture.rs` enforce the module-privacy boundary, pin the ratchet counts, guard against new `map_err(|_ident| ...)` source-discarding closures elsewhere in the crate, and confirm E124's "reapprove" guidance isn't copied onto any of the new variants.

No behavior change beyond E124's Display text gaining a reason suffix.
