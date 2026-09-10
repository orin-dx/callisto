---
callisto-graph: patch
---

**`release_execution.rs` reports its real artifact-manifest failure instead of a generic stale-intent error**

PR2 of the 4-PR `SPEC-ARCH-RELEASE-ERROR-TAXONOMY` migration (PR1: #79). `execute_release_with_artifacts`'s two `GraphError::ReleaseIntentStale` sites now report their real cause: a manifest that fails `validate_for_intent` surfaces the existing `GraphError::ArtifactManifest { source }` (E136) with the specific `ArtifactManifestError` cause (duplicate slot, mismatched slot roster, mismatched attestation, mismatched intent); an intent that declares artifact slots but receives no manifest at all surfaces `GraphError::ReleasePreconditionUnmet { requirement: ArtifactManifestProvided }` (E170). Extracted the decision into `require_artifact_manifest_matches_intent` for direct unit coverage (previously untested). The crate's `legacy_unclassified_ratchet` architecture test is updated: `release_execution.rs` is now at 0 (was 2), and the file is removed from the `map_err_ignore` evasion allowlist.
