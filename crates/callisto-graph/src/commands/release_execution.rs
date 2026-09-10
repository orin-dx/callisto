//! Pure reconciliation of a durable release execution state.
//!
//! This is deliberately separate from the executor: it observes neither Git,
//! registries, nor a forge. Its only authority is one exact intent and the
//! state explicitly bound to that intent.

use std::collections::{BTreeMap, BTreeSet};

use callisto_model::{
    ApplyPermit, ArtifactManifestV1, OperationState, ReleaseExecutionStateV1, ReleaseIntentV1, ReleaseOperationId,
};

use crate::{
    commands::{ReleaseStateStore, ReleaseStateWriter},
    GraphError,
};

use super::release::ValidatedReleaseIntent;
use crate::error::ReleasePreconditionRequirement;

/// Executes eligible operations one at a time with crash-safe state updates.
///
/// `Attempting` is persisted before dispatch. If dispatch returns an error,
/// the state deliberately remains `Attempting`: recovery must observe the
/// exact remote identity rather than guessing whether an effect occurred.
pub fn execute_release<W: ReleaseStateWriter>(
    capability: &ValidatedReleaseIntent<'_>,
    store: &ReleaseStateStore<W>,
    permit: &ApplyPermit,
) -> Result<ReleaseExecutionStateV1, GraphError> {
    execute_release_with_artifacts(capability, store, permit, None)
}

/// Executes a release after requiring the exact artifact manifest whenever
/// the intent declares compiled-binary slots.
pub fn execute_release_with_artifacts<W: ReleaseStateWriter>(
    capability: &ValidatedReleaseIntent<'_>,
    store: &ReleaseStateStore<W>,
    permit: &ApplyPermit,
    artifacts: Option<&ArtifactManifestV1>,
) -> Result<ReleaseExecutionStateV1, GraphError> {
    let intent = capability.intent();
    require_artifact_manifest_matches_intent(intent, artifacts)?;
    let mut state = store.load_or_initialize(intent, permit)?;
    loop {
        let Some(operation) = reconcile_release_execution(intent, &state)?.eligible().first().cloned() else {
            break;
        };
        capability.recheck_trust()?;
        state
            .mark_attempting(&operation)
            .map_err(|source| GraphError::ReleaseExecutionState { source })?;
        store.save(intent, &state, permit)?;

        let outcome = capability.dispatch_prepared(permit, &operation)?;
        state
            .mark_terminal(&operation, outcome)
            .map_err(|source| GraphError::ReleaseExecutionState { source })?;
        store.save(intent, &state, permit)?;
    }
    Ok(state)
}

/// Requires an exact artifact manifest whenever `intent` declares
/// compiled-binary artifact slots, and that a provided manifest actually
/// validates against `intent` when one is supplied regardless of slot
/// count (a manifest supplied for a slot-less intent must still be a real,
/// intent-bound manifest, not silently ignored).
fn require_artifact_manifest_matches_intent(
    intent: &ReleaseIntentV1,
    artifacts: Option<&ArtifactManifestV1>,
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
    state: &ReleaseExecutionStateV1,
) -> Result<ReconciledReleaseExecution, GraphError> {
    state
        .validate_for_intent(intent)
        .map_err(|source| GraphError::ReleaseExecutionState { source })?;

    let prerequisites: BTreeMap<_, _> = intent
        .operations
        .iter()
        .map(|operation| (operation.id().clone(), operation.prerequisites().to_vec()))
        .collect();

    let mut eligible = Vec::new();
    for operation in &intent.operations {
        if state.operation_state(operation.id()) != Some(OperationState::Pending) {
            continue;
        }
        let mut visited = BTreeSet::new();
        if prerequisites_satisfied_transitively(operation.id(), &prerequisites, state, &mut visited) {
            eligible.push(operation.id().clone());
        }
    }
    Ok(ReconciledReleaseExecution { eligible })
}

fn prerequisites_satisfied_transitively(
    id: &ReleaseOperationId,
    prerequisites: &BTreeMap<ReleaseOperationId, Vec<ReleaseOperationId>>,
    state: &ReleaseExecutionStateV1,
    visited: &mut BTreeSet<ReleaseOperationId>,
) -> bool {
    // `ReleaseIntentV1` has already proved this is a DAG. Keeping the visited
    // guard makes this helper fail closed if a future model version violates
    // that invariant rather than recursing indefinitely.
    if !visited.insert(id.clone()) {
        return false;
    }
    let result = prerequisites.get(id).is_some_and(|direct| {
        direct.iter().all(|prerequisite| {
            matches!(
                state.operation_state(prerequisite),
                Some(OperationState::Published | OperationState::AlreadySatisfied)
            ) && prerequisites_satisfied_transitively(prerequisite, prerequisites, state, visited)
        })
    });
    visited.remove(id);
    result
}

#[cfg(test)]
mod tests {
    use callisto_model::{
        Ecosystem, ExecutionTrustProfileV1, OperationBlockReason, OperationOutcome, RegistryBindingDigest,
        RegistryBindingId, ReleaseInputSnapshotV1, ReleaseOperation, ReleasePackageId, SourceIdentity, Version,
    };

    use super::*;

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
                    source_commit: callisto_model::CommitSha::parse(&"a".repeat(40)).unwrap(),
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
        let mut state = ReleaseExecutionStateV1::pending(&intent);
        let publish = intent.operations[0].id().clone();
        let tag = intent.operations[1].id().clone();
        let forge = intent.operations[2].id().clone();

        assert_eq!(
            reconcile_release_execution(&intent, &state).unwrap().eligible(),
            std::slice::from_ref(&publish)
        );
        state.mark_attempting(&publish).unwrap();
        state.mark_terminal(&publish, OperationOutcome::Published).unwrap();
        assert_eq!(
            reconcile_release_execution(&intent, &state).unwrap().eligible(),
            std::slice::from_ref(&tag)
        );
        state.mark_attempting(&tag).unwrap();
        state.mark_terminal(&tag, OperationOutcome::AlreadySatisfied).unwrap();
        assert_eq!(
            reconcile_release_execution(&intent, &state).unwrap().eligible(),
            &[forge]
        );
    }

    #[test]
    fn non_successful_or_attempting_prerequisites_never_make_dependents_eligible() {
        let intent = intent();
        let publish = intent.operations[0].id().clone();
        let tag = intent.operations[1].id().clone();
        let mut state = ReleaseExecutionStateV1::pending(&intent);
        state.mark_attempting(&publish).unwrap();
        assert!(reconcile_release_execution(&intent, &state)
            .unwrap()
            .eligible()
            .is_empty());
        state
            .mark_terminal(
                &publish,
                OperationOutcome::Blocked {
                    reason: OperationBlockReason::IndeterminateAttempt,
                },
            )
            .unwrap();
        assert!(reconcile_release_execution(&intent, &state)
            .unwrap()
            .eligible()
            .is_empty());
        assert_eq!(state.operation_state(&tag), Some(OperationState::Pending));
    }

    #[test]
    fn direct_success_cannot_hide_an_unsatisfied_transitive_prerequisite() {
        let intent = intent();
        let publish = intent.operations[0].id().clone();
        let tag = intent.operations[1].id().clone();
        let forge = intent.operations[2].id().clone();
        let mut state = ReleaseExecutionStateV1::pending(&intent);

        // The model state machine deliberately does not own DAG policy. This
        // represents a corrupted or legacy executor having recorded tag
        // success before its publish prerequisite. Reconciliation must still
        // fail closed for the forge operation.
        state.mark_attempting(&tag).unwrap();
        state.mark_terminal(&tag, OperationOutcome::Published).unwrap();
        assert_eq!(state.operation_state(&publish), Some(OperationState::Pending));
        let reconciled = reconcile_release_execution(&intent, &state).unwrap();
        assert_eq!(reconciled.eligible(), &[publish]);
        assert!(!reconciled.eligible().contains(&forge));
    }
}
