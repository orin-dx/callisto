//! Pure reconciliation of a durable release execution state.
//!
//! This is deliberately separate from the executor: it observes neither Git,
//! registries, nor a forge. Its only authority is one exact intent and the
//! state explicitly bound to that intent.

use std::collections::{BTreeMap, BTreeSet};

use callisto_model::{OperationState, ReleaseExecutionStateV1, ReleaseIntentV1, ReleaseOperationId};

use crate::GraphError;

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
        )
        .unwrap()
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
