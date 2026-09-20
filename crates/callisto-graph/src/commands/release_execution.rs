//! Pure reconciliation of a durable release execution state.
//!
//! This is deliberately separate from the executor: it observes neither Git,
//! registries, nor a forge. Its only authority is one exact intent and the
//! state explicitly bound to that intent.

use std::collections::{BTreeMap, BTreeSet};

use callisto_model::{
    ApplyPermit, OperationEvent, OperationState, ProviderObservationV1, ReleaseExecutionStateV1, ReleaseIntentV1,
    ReleaseOperationId, ReleaseRunEnvelopeV1, ReleaseRunKindV1,
};

use crate::{
    commands::{ReleaseStateStore, ReleaseStateWriter},
    GraphError,
};

use super::release::{ReleasePreflight, ValidatedReleaseIntent};
use super::release_artifacts::VerifiedArtifactManifest;
use crate::error::ReleasePreconditionRequirement;

/// Executes eligible operations one at a time with crash-safe state updates.
///
/// The run `envelope` was created and validated before this call, so the very
/// first thing persisted already names the coordinator, release source,
/// profile, intent, and artifact manifest of this run. State left behind by a
/// different run is rejected rather than adopted.
///
/// Each operation is observed once before `Attempting` is persisted, so a
/// pre-effect failure leaves it `Pending`. `Attempting` is persisted only
/// immediately before a mutating command; if dispatch then returns an error,
/// the state deliberately remains `Attempting`: recovery must observe the
/// exact remote identity rather than guessing whether an effect occurred.
pub fn execute_release<W: ReleaseStateWriter>(
    capability: &ValidatedReleaseIntent<'_>,
    store: &ReleaseStateStore<W>,
    permit: &ApplyPermit,
    envelope: &ReleaseRunEnvelopeV1,
    artifacts: Option<&VerifiedArtifactManifest<'_>>,
) -> Result<ReleaseExecutionStateV1, GraphError> {
    let intent = capability.intent();
    require_verified_artifact_manifest(intent, artifacts)?;
    envelope
        .validate_for_intent(intent)
        .map_err(|source| GraphError::ReleaseRunEnvelope { source })?;
    let (mut state, state_was_missing) = match store.load(intent, envelope)? {
        Some(state) => (state, false),
        None => {
            let state = ReleaseExecutionStateV1::new(intent, envelope.clone())
                .map_err(|source| GraphError::ReleaseExecutionState { source })?;
            store.save(intent, &state, permit)?;
            (state, true)
        }
    };
    if envelope.kind() == ReleaseRunKindV1::Recovery && state_was_missing {
        reconstruct_missing_state(capability, store, permit, &mut state, artifacts)?;
    }
    recover_interrupted_operations(capability, store, permit, &mut state, artifacts)?;
    loop {
        let Some(operation) = reconcile_release_execution(intent, Some(&state))?
            .eligible()
            .first()
            .cloned()
        else {
            break;
        };
        capability.recheck_trust()?;
        // Observe before persisting `Attempting`: a conflict, indeterminate
        // provider, or failed observation raised here issued no effect, so the
        // operation must stay `Pending` and remain rerunnable.
        let event = match capability.preflight_prepared(&operation, artifacts)? {
            ReleasePreflight::AlreadySatisfied { evidence } => OperationEvent::ObservedExactBeforeEffect { evidence },
            ReleasePreflight::Proceed { proof } => {
                apply(&mut state, &operation, &OperationEvent::Attempt { proof })?;
                store.save(intent, &state, permit)?;
                OperationEvent::Confirmed {
                    evidence: capability.dispatch_prepared(permit, &operation, artifacts)?,
                }
            }
        };
        apply(&mut state, &operation, &event)?;
        store.save(intent, &state, permit)?;
    }
    require_terminal_success(intent, &state)?;
    Ok(state)
}

/// The single seam through which this executor changes durable state.
fn apply(
    state: &mut ReleaseExecutionStateV1,
    operation: &ReleaseOperationId,
    event: &OperationEvent,
) -> Result<(), GraphError> {
    state
        .apply(operation, event)
        .map(|_| ())
        .map_err(|source| GraphError::ReleaseExecutionState { source })
}

/// Reconstructs only exact effects after an operator explicitly chose the
/// recovery lifecycle. `Absent`, `Conflict`, and `Indeterminate` are not
/// state transitions: the former remains eligible for normal dispatch once
/// its prerequisites are reconstructed, while the latter two stop recovery.
fn reconstruct_missing_state<W: ReleaseStateWriter>(
    capability: &ValidatedReleaseIntent<'_>,
    store: &ReleaseStateStore<W>,
    permit: &ApplyPermit,
    state: &mut ReleaseExecutionStateV1,
    artifacts: Option<&VerifiedArtifactManifest<'_>>,
) -> Result<(), GraphError> {
    let pending: Vec<_> = capability
        .intent()
        .operations
        .iter()
        .filter(|operation| state.operation_state(operation.id()) == Some(OperationState::Pending))
        .map(|operation| operation.id().clone())
        .collect();
    for operation in pending {
        capability.recheck_trust()?;
        reconstruct_missing_operation(state, &operation, capability.observe_prepared(&operation, artifacts)?)?;
        store.save(capability.intent(), state, permit)?;
    }
    Ok(())
}

fn reconstruct_missing_operation(
    state: &mut ReleaseExecutionStateV1,
    operation: &ReleaseOperationId,
    observation: ProviderObservationV1,
) -> Result<(), GraphError> {
    match &observation {
        ProviderObservationV1::Exact { .. } => apply(
            state,
            operation,
            &OperationEvent::AdoptedExact {
                evidence: observation
                    .exact_evidence()
                    .expect("an exact observation mints its evidence"),
            },
        ),
        ProviderObservationV1::Absent => Ok(()),
        ProviderObservationV1::Conflict { .. } | ProviderObservationV1::Indeterminate { .. } => {
            Err(GraphError::ReleaseRecoveryUnresolved {
                operation: Box::new(operation.clone()),
                observation: Box::new(observation),
            })
        }
    }
}

/// Reconciles only persisted `Attempting` operations before a rerun can issue
/// any effect. `Attempting` is deliberately not downgraded to `Pending`:
/// after a process crash, an absent or unavailable provider response cannot
/// prove that a previous request did not take effect. Exact observation is
/// the sole automatic convergence path.
fn recover_interrupted_operations<W: ReleaseStateWriter>(
    capability: &ValidatedReleaseIntent<'_>,
    store: &ReleaseStateStore<W>,
    permit: &ApplyPermit,
    state: &mut ReleaseExecutionStateV1,
    artifacts: Option<&VerifiedArtifactManifest<'_>>,
) -> Result<(), GraphError> {
    let attempting: Vec<_> = capability
        .intent()
        .operations
        .iter()
        .filter(|operation| state.operation_state(operation.id()) == Some(OperationState::Attempting))
        .map(|operation| operation.id().clone())
        .collect();
    for operation in attempting {
        capability.recheck_trust()?;
        recover_interrupted_operation(state, &operation, capability.observe_prepared(&operation, artifacts)?)?;
        store.save(capability.intent(), state, permit)?;
    }
    Ok(())
}

fn recover_interrupted_operation(
    state: &mut ReleaseExecutionStateV1,
    operation: &ReleaseOperationId,
    observation: ProviderObservationV1,
) -> Result<(), GraphError> {
    let Some(evidence) = observation.exact_evidence() else {
        return Err(GraphError::ReleaseRecoveryUnresolved {
            operation: Box::new(operation.clone()),
            observation: Box::new(observation),
        });
    };
    apply(state, operation, &OperationEvent::RecoveredExact { evidence })
}

/// Refuses to report success until every operation has an observed terminal
/// success state. A quiescent `Attempting` operation is an interrupted effect,
/// not evidence that the release completed.
fn require_terminal_success(intent: &ReleaseIntentV1, state: &ReleaseExecutionStateV1) -> Result<(), GraphError> {
    let count = intent
        .operations
        .iter()
        .filter(|operation| {
            !matches!(
                state.operation_state(operation.id()),
                Some(OperationState::Published | OperationState::AlreadySatisfied)
            )
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

/// The exact pending operations which are safe for a future executor to
/// consider. Being listed here does not perform an effect or bypass its
/// immediately-before-effect revalidation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciledReleaseExecution {
    eligible: Vec<ReleaseOperationId>,
}

impl ReconciledReleaseExecution {
    /// Operations in the intent's stable canonical order.
    pub fn eligible(&self) -> &[ReleaseOperationId] {
        &self.eligible
    }
}

/// Reconciles `state` against exactly `intent` without performing any I/O.
///
/// An operation is eligible only while it is `Pending` and every *transitive*
/// prerequisite is exactly `Published` or `AlreadySatisfied`. `Attempting`,
/// missing, failed, and blocked operations are never inferred as success.
pub fn reconcile_release_execution(
    intent: &ReleaseIntentV1,
    state: Option<&ReleaseExecutionStateV1>,
) -> Result<ReconciledReleaseExecution, GraphError> {
    if let Some(state) = state {
        state
            .validate_for_intent(intent)
            .map_err(|source| GraphError::ReleaseExecutionState { source })?;
    }

    let prerequisites: BTreeMap<_, _> = intent
        .operations
        .iter()
        .map(|operation| (operation.id().clone(), operation.prerequisites().to_vec()))
        .collect();

    let mut eligible = Vec::new();
    let mut satisfied = BTreeMap::new();
    for operation in &intent.operations {
        if operation_state(state, operation.id()) != Some(OperationState::Pending) {
            continue;
        }
        let mut on_path = BTreeSet::new();
        if prerequisites_satisfied_transitively(operation.id(), &prerequisites, state, &mut on_path, &mut satisfied) {
            eligible.push(operation.id().clone());
        }
    }
    Ok(ReconciledReleaseExecution { eligible })
}

/// `satisfied` memoizes each subtree's answer across the whole reconcile, so
/// the walk is linear in the DAG rather than enumerating every path.
/// A run with no journal yet has every operation `Pending`.
fn operation_state(state: Option<&ReleaseExecutionStateV1>, id: &ReleaseOperationId) -> Option<OperationState> {
    match state {
        Some(state) => state.operation_state(id),
        None => Some(OperationState::Pending),
    }
}

fn prerequisites_satisfied_transitively(
    id: &ReleaseOperationId,
    prerequisites: &BTreeMap<ReleaseOperationId, Vec<ReleaseOperationId>>,
    state: Option<&ReleaseExecutionStateV1>,
    on_path: &mut BTreeSet<ReleaseOperationId>,
    satisfied: &mut BTreeMap<ReleaseOperationId, bool>,
) -> bool {
    if let Some(known) = satisfied.get(id) {
        return *known;
    }
    // `ReleaseIntentV1` has already proved this is a DAG. Keeping the on-path
    // guard makes this helper fail closed if a future model version violates
    // that invariant rather than recursing indefinitely. A cycle's answer is
    // deliberately not memoized: it is a property of this path, not of `id`.
    if !on_path.insert(id.clone()) {
        return false;
    }
    let result = prerequisites.get(id).is_some_and(|direct| {
        direct.iter().all(|prerequisite| {
            matches!(
                operation_state(state, prerequisite),
                Some(OperationState::Published | OperationState::AlreadySatisfied)
            ) && prerequisites_satisfied_transitively(prerequisite, prerequisites, state, on_path, satisfied)
        })
    });
    on_path.remove(id);
    satisfied.insert(id.clone(), result);
    result
}

#[cfg(test)]
mod tests {
    use callisto_model::{
        Ecosystem, ExecutionTrustProfileV1, OperationBlockReason, ProviderConflictReason, ProviderIndeterminateCause,
        RegistryBindingDigest, RegistryBindingId, ReleaseInputSnapshotV1, ReleaseOperation, ReleasePackageId,
        SourceIdentity, Version,
    };

    use super::*;
    use crate::commands::release_test_support::{
        already_satisfied_event, attempt_event, exact, pending_state, publish_operation, recovery_state,
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
            callisto_model::ReleaseProfileId::production(),
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
            callisto_model::ReleaseProfileId::production(),
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

        assert_eq!(
            reconcile_release_execution(&intent, Some(&state)).unwrap().eligible(),
            std::slice::from_ref(&publish)
        );
        publish_operation(&mut state, &publish);
        assert_eq!(
            reconcile_release_execution(&intent, Some(&state)).unwrap().eligible(),
            std::slice::from_ref(&tag)
        );
        state.apply(&tag, &already_satisfied_event(&tag)).unwrap();
        assert_eq!(
            reconcile_release_execution(&intent, Some(&state)).unwrap().eligible(),
            &[forge]
        );
    }

    #[test]
    fn non_successful_or_attempting_prerequisites_never_make_dependents_eligible() {
        let intent = intent();
        let publish = intent.operations[0].id().clone();
        let tag = intent.operations[1].id().clone();
        let mut state = pending_state(&intent);
        state.apply(&publish, &attempt_event()).unwrap();
        assert!(reconcile_release_execution(&intent, Some(&state))
            .unwrap()
            .eligible()
            .is_empty());
        state
            .apply(
                &publish,
                &OperationEvent::Blocked {
                    reason: OperationBlockReason::IndeterminateAttempt,
                },
            )
            .unwrap();
        assert!(reconcile_release_execution(&intent, Some(&state))
            .unwrap()
            .eligible()
            .is_empty());
        assert_eq!(state.operation_state(&tag), Some(OperationState::Pending));
    }

    #[test]
    fn interrupted_operation_converges_only_from_an_exact_provider_observation() {
        let intent = intent();
        let operation = intent.operations[0].id().clone();
        let mut state = pending_state(&intent);
        state.apply(&operation, &attempt_event()).unwrap();

        // An interrupted attempt that is now exact is our own effect landing,
        // which is a different fact from a pre-existing one.
        recover_interrupted_operation(&mut state, &operation, exact(&operation)).unwrap();
        assert_eq!(state.operation_state(&operation), Some(OperationState::Published));

        let mut unresolved = pending_state(&intent);
        unresolved.apply(&operation, &attempt_event()).unwrap();
        let error =
            recover_interrupted_operation(&mut unresolved, &operation, ProviderObservationV1::Absent).unwrap_err();
        assert!(matches!(
            error,
            GraphError::ReleaseRecoveryUnresolved { ref observation, .. }
                if **observation == ProviderObservationV1::Absent
        ));
        assert_eq!(unresolved.operation_state(&operation), Some(OperationState::Attempting));
    }

    #[test]
    fn missing_state_reconstruction_only_adopts_exact_provider_effects() {
        let intent = intent();
        let operation = intent.operations[0].id().clone();
        let mut state = recovery_state(&intent);

        reconstruct_missing_operation(&mut state, &operation, exact(&operation)).unwrap();
        assert_eq!(
            state.operation_state(&operation),
            Some(OperationState::AlreadySatisfied)
        );

        let mut absent = recovery_state(&intent);
        reconstruct_missing_operation(&mut absent, &operation, ProviderObservationV1::Absent).unwrap();
        assert_eq!(absent.operation_state(&operation), Some(OperationState::Pending));

        for observation in [
            ProviderObservationV1::Conflict {
                reason: ProviderConflictReason::RemoteTagTargetDiffers,
            },
            ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::CommandFailed,
            },
        ] {
            let mut unresolved = recovery_state(&intent);
            let error = reconstruct_missing_operation(&mut unresolved, &operation, observation.clone()).unwrap_err();
            assert!(matches!(
                error,
                GraphError::ReleaseRecoveryUnresolved { observation: ref found, .. } if **found == observation
            ));
            assert_eq!(unresolved.operation_state(&operation), Some(OperationState::Pending));
        }

        // A normal run has no adoption privilege at all.
        let mut initial = pending_state(&intent);
        assert!(matches!(
            reconstruct_missing_operation(&mut initial, &operation, exact(&operation)),
            Err(GraphError::ReleaseExecutionState { .. })
        ));
    }

    #[test]
    fn direct_success_cannot_hide_an_unsatisfied_transitive_prerequisite() {
        let intent = intent();
        let publish = intent.operations[0].id().clone();
        let tag = intent.operations[1].id().clone();
        let forge = intent.operations[2].id().clone();
        let mut state = pending_state(&intent);

        // The model state machine deliberately does not own DAG policy. This
        // represents a corrupted or legacy executor having recorded tag
        // success before its publish prerequisite. Reconciliation must still
        // fail closed for the forge operation.
        publish_operation(&mut state, &tag);
        assert_eq!(state.operation_state(&publish), Some(OperationState::Pending));
        let reconciled = reconcile_release_execution(&intent, Some(&state)).unwrap();
        assert_eq!(reconciled.eligible(), &[publish]);
        assert!(!reconciled.eligible().contains(&forge));
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
