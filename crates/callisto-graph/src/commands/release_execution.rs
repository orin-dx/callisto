//! The release executor.
//!
//! Every run starts from a fresh in-memory state and observes each provider
//! before acting, so a rerun adopts whatever an earlier run already landed.

use std::collections::{BTreeMap, BTreeSet};

use callisto_model::{
    ApplyPermit, OperationEvent, OperationState, ReleaseExecutionStateV1, ReleaseIntentV1, ReleaseOperationId,
    ReleaseOperationRole, ReleaseRunEnvelopeV1, ReleaseStateError,
};

use crate::GraphError;

use super::release::{ReleasePreflight, ReleaseProviderSet};
use super::release_artifacts::VerifiedArtifactManifest;
use crate::error::ReleasePreconditionRequirement;

/// Executes eligible operations one at a time.
///
/// The run `envelope` was created and validated before this call. Each
/// operation is observed once before any effect: an exact observation is
/// adopted as already satisfied, an absent one is published and confirmed,
/// and anything else stops the run before an effect is issued.
pub fn execute_release<P: ReleaseProviderSet + ?Sized>(
    capability: &P,
    permit: &ApplyPermit,
    envelope: &ReleaseRunEnvelopeV1,
    artifacts: Option<&VerifiedArtifactManifest<'_>>,
) -> Result<ReleaseExecutionStateV1, GraphError> {
    let intent = capability.intent();
    require_verified_artifact_manifest(intent, artifacts)?;
    envelope
        .validate_for_intent(intent)
        .map_err(|source| GraphError::ReleaseRunEnvelope { source })?;
    let mut state = ReleaseExecutionStateV1::new(intent, envelope.clone())
        .map_err(|source| GraphError::ReleaseExecutionState { source })?;
    while let Some(operation) = eligible_operations(intent, &state).first().cloned() {
        capability.recheck_trust()?;
        let event = match capability.preflight(&operation, artifacts)? {
            ReleasePreflight::AlreadySatisfied { evidence } => {
                warn_if_registry_version_adopted(&operation);
                OperationEvent::ObservedExactBeforeEffect { evidence }
            }
            ReleasePreflight::Proceed { proof } => {
                let attempt = OperationEvent::Attempt { proof };
                apply(&mut state, &operation, &attempt)?;
                // The proof stays borrowed from the recorded attempt: only an
                // attempt may authorize the effect that follows it.
                let OperationEvent::Attempt { proof } = &attempt else {
                    return Err(GraphError::ReleaseInvariant {
                        detail: "the attempt event just built is not an Attempt".to_owned(),
                    });
                };
                OperationEvent::Confirmed {
                    evidence: capability.publish(permit, proof, &operation, artifacts)?,
                }
            }
        };
        apply(&mut state, &operation, &event)?;
    }
    require_terminal_success(intent, &state)?;
    Ok(state)
}

fn warn_if_registry_version_adopted(operation: &ReleaseOperationId) {
    let package = match &operation.role {
        ReleaseOperationRole::RegistryPublish { .. } => &operation.package,
        ReleaseOperationRole::PlatformPublish { platform, .. } => platform.name(),
        _ => return,
    };
    eprintln!(
        "warning: {package} {} is already published; skipping",
        operation.version
    );
}

/// The single seam through which this executor changes state.
fn apply(
    state: &mut ReleaseExecutionStateV1,
    operation: &ReleaseOperationId,
    event: &OperationEvent,
) -> Result<(), GraphError> {
    state
        .apply(operation, event)
        .map(|_| ())
        .map_err(|source| match source {
            ReleaseStateError::EvidenceRejected(source) => GraphError::ReleaseProviderObservation { source },
            source => GraphError::ReleaseExecutionState { source },
        })
}

/// Refuses to report success until every operation has an observed terminal
/// success state.
fn require_terminal_success(intent: &ReleaseIntentV1, state: &ReleaseExecutionStateV1) -> Result<(), GraphError> {
    let count = intent
        .operations
        .iter()
        .filter(|operation| {
            !state
                .operation_state(operation.id())
                .is_some_and(OperationState::is_success)
        })
        .count();
    if count == 0 {
        Ok(())
    } else {
        Err(GraphError::ReleaseIncomplete { count })
    }
}

/// Requires the verification capability whenever `intent` declares compiled
/// binary slots. Its private constructor already proved both manifest binding
/// and local byte/attestation identity.
fn require_verified_artifact_manifest(
    intent: &ReleaseIntentV1,
    artifacts: Option<&VerifiedArtifactManifest<'_>>,
) -> Result<(), GraphError> {
    match (intent.artifact_slots.is_empty(), artifacts) {
        (true, None) => Ok(()),
        (true, Some(_)) => Err(GraphError::ReleaseInvariant {
            detail: "verified artifact manifest supplied for slot-less release intent".to_owned(),
        }),
        (false, Some(artifacts)) => require_artifact_manifest_matches_intent(intent, Some(artifacts.manifest())),
        (false, None) => Err(GraphError::ReleasePreconditionUnmet {
            requirement: ReleasePreconditionRequirement::VerifiedArtifactManifest,
        }),
    }
}

/// Validates the serializable manifest independently of local byte and
/// attestation verification. Kept as a focused invariant helper for tests;
/// production execution requires `VerifiedArtifactManifest` above.
fn require_artifact_manifest_matches_intent(
    intent: &ReleaseIntentV1,
    artifacts: Option<&callisto_model::ArtifactManifestV1>,
) -> Result<(), GraphError> {
    match (intent.artifact_slots.is_empty(), artifacts) {
        (true, None) => Ok(()),
        (true, Some(manifest)) | (false, Some(manifest)) => manifest
            .validate_for_intent(intent)
            .map_err(|source| GraphError::ArtifactManifest { source }),
        (false, None) => Err(GraphError::ReleasePreconditionUnmet {
            requirement: ReleasePreconditionRequirement::ArtifactManifestProvided,
        }),
    }
}

/// The pending operations whose every transitive prerequisite is exactly
/// `Published` or `AlreadySatisfied`, in the intent's canonical order.
pub(crate) fn eligible_operations(
    intent: &ReleaseIntentV1,
    state: &ReleaseExecutionStateV1,
) -> Vec<ReleaseOperationId> {
    let prerequisites: BTreeMap<_, _> = intent
        .operations
        .iter()
        .map(|operation| (operation.id().clone(), operation.prerequisites().to_vec()))
        .collect();
    let mut satisfied = BTreeMap::new();
    intent
        .operations
        .iter()
        .filter(|operation| state.operation_state(operation.id()) == Some(OperationState::Pending))
        .filter(|operation| {
            prerequisites_satisfied_transitively(
                operation.id(),
                &prerequisites,
                state,
                &mut BTreeSet::new(),
                &mut satisfied,
            )
        })
        .map(|operation| operation.id().clone())
        .collect()
}

/// `satisfied` memoizes each subtree's answer, so the walk is linear in the DAG.
fn prerequisites_satisfied_transitively(
    id: &ReleaseOperationId,
    prerequisites: &BTreeMap<ReleaseOperationId, Vec<ReleaseOperationId>>,
    state: &ReleaseExecutionStateV1,
    on_path: &mut BTreeSet<ReleaseOperationId>,
    satisfied: &mut BTreeMap<ReleaseOperationId, bool>,
) -> bool {
    if let Some(known) = satisfied.get(id) {
        return *known;
    }
    // The intent is already a proven DAG; the on-path guard fails closed on a cycle anyway.
    if !on_path.insert(id.clone()) {
        return false;
    }
    let result = prerequisites.get(id).is_some_and(|direct| {
        direct.iter().all(|prerequisite| {
            state
                .operation_state(prerequisite)
                .is_some_and(OperationState::is_success)
                && prerequisites_satisfied_transitively(prerequisite, prerequisites, state, on_path, satisfied)
        })
    });
    on_path.remove(id);
    satisfied.insert(id.clone(), result);
    result
}

#[cfg(test)]
mod tests {
    use callisto_model::{
        Ecosystem, ExecutionTrustProfileV1, OperationBlockReason, RegistryBindingDigest, RegistryBindingId,
        ReleaseInputSnapshotV1, ReleaseOperation, ReleasePackageId, SourceIdentity, Version,
    };

    use super::*;
    use crate::commands::release_test_support::{
        already_satisfied_event, attempt_event, pending_state, publish_operation,
    };

    fn intent() -> ReleaseIntentV1 {
        let package = ReleasePackageId::new(Ecosystem::Cargo, "demo").unwrap();
        let version = Version::semver(1, 0, 0);
        let registry = RegistryBindingId::new(
            "crates",
            RegistryBindingDigest::from_normalized_binding(b"crates-default"),
        )
        .unwrap();
        let publish = ReleaseOperation::registry_publish(package.clone(), version.clone(), registry, vec![]).unwrap();
        let tag = ReleaseOperation::tag(package.clone(), version.clone(), vec![publish.id().clone()]).unwrap();
        let forge = ReleaseOperation::forge_release(package.clone(), version.clone(), vec![tag.id().clone()]).unwrap();
        ReleaseIntentV1::new(
            callisto_model::ReleaseDecisionV1::new(vec![callisto_model::ReleaseDecisionEntry {
                package: package.clone(),
                target_version: version.clone(),
                reasons: vec![callisto_model::ReleaseInclusionReason::ExplicitSelection],
            }])
            .unwrap(),
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]).unwrap(),
            ExecutionTrustProfileV1::GitCommit,
            vec![publish, tag, forge],
            vec![],
        )
        .unwrap()
    }

    fn artifact_slot(package: &ReleasePackageId, version: &Version) -> callisto_model::ArtifactSlotId {
        callisto_model::ArtifactSlotId::new(
            package.clone(),
            version.clone(),
            "x86_64-unknown-linux-gnu",
            "demo.tar.gz",
            callisto_model::GitHubRepository::parse("orin-dx/callisto").unwrap(),
            ".github/workflows/release.yml",
            callisto_model::CommitSha::parse(&"b".repeat(40)).unwrap(),
        )
        .unwrap()
    }

    /// An intent that declares exactly one compiled-binary artifact slot.
    fn intent_with_artifact_slot() -> (ReleaseIntentV1, callisto_model::ArtifactSlotId) {
        intent_with_artifact_slot_named("demo")
    }

    fn intent_with_artifact_slot_named(name: &str) -> (ReleaseIntentV1, callisto_model::ArtifactSlotId) {
        let package = ReleasePackageId::new(Ecosystem::Cargo, name).unwrap();
        let version = Version::semver(1, 0, 0);
        let slot = artifact_slot(&package, &version);
        let upload = ReleaseOperation::artifact_upload(slot.clone(), vec![]).unwrap();
        let intent = ReleaseIntentV1::new(
            callisto_model::ReleaseDecisionV1::new(vec![callisto_model::ReleaseDecisionEntry {
                package: package.clone(),
                target_version: version.clone(),
                reasons: vec![callisto_model::ReleaseInclusionReason::ExplicitSelection],
            }])
            .unwrap(),
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]).unwrap(),
            ExecutionTrustProfileV1::GitCommit,
            vec![upload],
            vec![slot.clone()],
        )
        .unwrap();
        (intent, slot)
    }

    /// A manifest that validates against `intent`.
    fn matching_artifact_manifest(
        intent: &ReleaseIntentV1,
        slot: callisto_model::ArtifactSlotId,
    ) -> callisto_model::ArtifactManifestV1 {
        let digest = callisto_model::ArtifactDigest::from_bytes(b"binary");
        callisto_model::ArtifactManifestV1::new(
            intent,
            vec![callisto_model::ArtifactManifestEntryV1 {
                slot,
                digest: digest.clone(),
                byte_length: 6,
                attestation: callisto_model::GitHubArtifactAttestationV1 {
                    repository: callisto_model::GitHubRepository::parse("orin-dx/callisto").unwrap(),
                    workflow_path: ".github/workflows/release.yml".to_string(),
                    workflow_commit: callisto_model::CommitSha::parse(&"b".repeat(40)).unwrap(),
                    subject_digest: digest,
                    source_commit: callisto_model::CommitSha::parse(&"b".repeat(40)).unwrap(),
                },
            }],
        )
        .unwrap()
    }

    /// Regression coverage for the confirmed bug: an intent declaring
    /// artifact slots but given no manifest at all must fail with a real,
    /// specific precondition -- not the unrelated `ReleaseIntentStale`.
    #[test]
    fn require_artifact_manifest_matches_intent_rejects_missing_manifest_for_slotted_intent() {
        let (intent, _slot) = intent_with_artifact_slot();
        let err = require_artifact_manifest_matches_intent(&intent, None).unwrap_err();
        assert!(
            matches!(
                err,
                GraphError::ReleasePreconditionUnmet {
                    requirement: ReleasePreconditionRequirement::ArtifactManifestProvided
                }
            ),
            "expected ReleasePreconditionUnmet(ArtifactManifestProvided), got {err:?}"
        );
    }

    /// Regression coverage: a manifest that fails validation (here, bound to
    /// a different intent) must surface the real `ArtifactManifestError`
    /// cause via `GraphError::ArtifactManifest`, not `ReleaseIntentStale`.
    #[test]
    fn require_artifact_manifest_matches_intent_rejects_a_manifest_that_fails_validation() {
        let (intent, slot) = intent_with_artifact_slot();
        let manifest = matching_artifact_manifest(&intent, slot);

        let (other_intent, _other_slot) = intent_with_artifact_slot_named("other-demo");
        let err = require_artifact_manifest_matches_intent(&other_intent, Some(&manifest)).unwrap_err();
        match err {
            GraphError::ArtifactManifest { source } => {
                assert_eq!(source, callisto_model::ArtifactManifestError::MismatchedIntent);
            }
            other => panic!("expected ArtifactManifest{{source: MismatchedIntent}}, got {other:?}"),
        }
    }

    /// A slot-less intent given no manifest must succeed (no behavior
    /// change from before this PR).
    #[test]
    fn require_artifact_manifest_matches_intent_accepts_no_manifest_for_slotless_intent() {
        let intent = intent();
        assert!(require_artifact_manifest_matches_intent(&intent, None).is_ok());
    }

    /// A slot-less intent given a valid, matching manifest must still
    /// succeed -- a manifest is never silently ignored just because the
    /// intent declared zero slots.
    #[test]
    fn require_artifact_manifest_matches_intent_accepts_a_valid_manifest_for_slotted_intent() {
        let (intent, slot) = intent_with_artifact_slot();
        let manifest = matching_artifact_manifest(&intent, slot);
        assert!(require_artifact_manifest_matches_intent(&intent, Some(&manifest)).is_ok());
    }

    #[test]
    fn reconciliation_requires_transitive_exact_successes() {
        let intent = intent();
        let mut state = pending_state(&intent);
        let publish = intent.operations[0].id().clone();
        let tag = intent.operations[1].id().clone();
        let forge = intent.operations[2].id().clone();

        assert_eq!(eligible_operations(&intent, &state), std::slice::from_ref(&publish));
        publish_operation(&mut state, &publish);
        assert_eq!(eligible_operations(&intent, &state), std::slice::from_ref(&tag));
        state.apply(&tag, &already_satisfied_event(&tag)).unwrap();
        assert_eq!(eligible_operations(&intent, &state), [forge]);
    }

    #[test]
    fn non_successful_or_attempting_prerequisites_never_make_dependents_eligible() {
        let intent = intent();
        let publish = intent.operations[0].id().clone();
        let tag = intent.operations[1].id().clone();
        let mut state = pending_state(&intent);
        state.apply(&publish, &attempt_event()).unwrap();
        assert!(eligible_operations(&intent, &state).is_empty());
        state
            .apply(
                &publish,
                &OperationEvent::Blocked {
                    reason: OperationBlockReason::IndeterminateAttempt,
                },
            )
            .unwrap();
        assert!(eligible_operations(&intent, &state).is_empty());
        assert_eq!(state.operation_state(&tag), Some(OperationState::Pending));
    }

    #[test]
    fn direct_success_cannot_hide_an_unsatisfied_transitive_prerequisite() {
        let intent = intent();
        let publish = intent.operations[0].id().clone();
        let tag = intent.operations[1].id().clone();
        let forge = intent.operations[2].id().clone();
        let mut state = pending_state(&intent);

        // The model state machine deliberately does not own DAG policy. This
        // represents a defective executor having recorded tag success before
        // its publish prerequisite. Eligibility must still fail closed for the
        // forge operation.
        publish_operation(&mut state, &tag);
        assert_eq!(state.operation_state(&publish), Some(OperationState::Pending));
        let eligible = eligible_operations(&intent, &state);
        assert_eq!(eligible, [publish]);
        assert!(!eligible.contains(&forge));
    }

    #[test]
    fn quiescent_incomplete_state_is_never_successful() {
        let intent = intent();
        let mut state = pending_state(&intent);
        state.apply(intent.operations[0].id(), &attempt_event()).unwrap();

        assert!(matches!(
            require_terminal_success(&intent, &state),
            Err(GraphError::ReleaseIncomplete { count: 3 })
        ));
    }

    #[test]
    fn every_verified_terminal_success_allows_completion() {
        let intent = intent();
        let mut state = pending_state(&intent);
        for operation in &intent.operations {
            state
                .apply(operation.id(), &already_satisfied_event(operation.id()))
                .unwrap();
        }

        assert!(require_terminal_success(&intent, &state).is_ok());
    }
}
