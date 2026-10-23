//! The release executor.
//!
//! Every run starts from a fresh in-memory state and observes each provider
//! before acting, so a rerun adopts whatever an earlier run already landed.

use std::collections::BTreeSet;
use std::time::Duration;

use michi_graph::{Dag, Order};

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
    let dag = operation_dag(intent)?;
    // Registry publishes issued but not yet seen on their registry; they stay `Attempting` until they are.
    let mut issued = BTreeSet::new();
    loop {
        let Some(operation) = eligible_in(&dag, &state, &issued).first().cloned() else {
            if issued.is_empty() {
                break;
            }
            settle_issued(capability, artifacts, &mut state, &mut issued)?;
            continue;
        };
        capability.recheck_trust()?;
        match capability.preflight(&operation, artifacts)? {
            ReleasePreflight::AlreadySatisfied { evidence } => {
                warn_if_registry_version_adopted(&operation);
                apply(
                    &mut state,
                    &operation,
                    &OperationEvent::ObservedExactBeforeEffect { evidence },
                )?;
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
                if capability.defers_confirmation(&operation) {
                    capability.issue(permit, proof, &operation, artifacts)?;
                    issued.insert(operation);
                } else {
                    let evidence = capability.publish(permit, proof, &operation, artifacts)?;
                    apply(&mut state, &operation, &OperationEvent::Confirmed { evidence })?;
                }
            }
        }
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
        (true, Some(artifacts)) if !callisto_model::python_publishes(intent).is_empty() => {
            require_artifact_manifest_matches_intent(intent, Some(artifacts.manifest()))
        }
        (true, Some(_)) => Err(GraphError::ReleaseInvariant {
            detail: "verified artifact manifest supplied for a release intent with nothing to seal".to_owned(),
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

/// The intent's operations as a DAG, built once per run. A constructed intent is already a proven DAG, so a failure
/// here means the intent was altered after validation.
pub(crate) fn operation_dag(intent: &ReleaseIntentV1) -> Result<Dag<ReleaseOperationId>, GraphError> {
    Dag::from_nodes(
        intent
            .operations
            .iter()
            .map(|operation| (operation.id().clone(), operation.prerequisites().to_vec())),
        Order::Key,
    )
    .map_err(|violations| GraphError::ReleaseInvariant {
        detail: format!("the release operations do not form a valid DAG: {violations}"),
    })
}

/// Looks at every issued operation until one is shown, so the run goes on with whatever that unlocks, or gives up
/// once the registries have had their whole window.
///
/// Each look is one observation per issued operation, and the issued operations are looked at together: the wait is
/// the slowest registry's, not the sum of them.
fn settle_issued<P: ReleaseProviderSet + ?Sized>(
    capability: &P,
    artifacts: Option<&VerifiedArtifactManifest<'_>>,
    state: &mut ReleaseExecutionStateV1,
    issued: &mut BTreeSet<ReleaseOperationId>,
) -> Result<(), GraphError> {
    let window = issued
        .iter()
        .map(|id| capability.confirmation_window(id))
        .max()
        .unwrap_or_default();
    let mut waited = Duration::ZERO;
    let mut step = 0;
    loop {
        let mut progressed = false;
        for id in issued.clone() {
            if let Some(evidence) = capability.settle(&id, artifacts)? {
                apply(state, &id, &OperationEvent::Confirmed { evidence })?;
                issued.remove(&id);
                progressed = true;
            }
        }
        if issued.is_empty() || progressed {
            return Ok(());
        }
        let remaining = window.saturating_sub(waited);
        if remaining.is_zero() {
            let mut unconfirmed = issued.iter();
            let Some(first) = unconfirmed.next() else {
                return Ok(());
            };
            for other in unconfirmed {
                eprintln!("warning: {other} is also still not shown by its registry");
            }
            return Err(GraphError::RegistryPublishUnconfirmed {
                package: first.package.name().to_owned(),
                version: first.version.clone(),
                waited: window,
            });
        }
        let wait = crate::commands::release::provider::policy::visibility_backoff(step).min(remaining);
        capability.pause(wait);
        waited += wait;
        step += 1;
    }
}

/// A registry publish, which is issued now and shown later.
fn is_registry_effect(id: &ReleaseOperationId) -> bool {
    matches!(
        id.role,
        ReleaseOperationRole::RegistryPublish { .. } | ReleaseOperationRole::PlatformPublish { .. }
    )
}

/// The pending operations that may start, in the intent's canonical order.
///
/// An operation may start when every one of its transitive prerequisites is `Published` or `AlreadySatisfied`. A
/// registry publish may also follow a registry publish that is only `issued`: npm takes a package whose dependency is
/// not visible yet, and everything announced (tag, release, uploads, publication) still waits for the version to be
/// shown. `ready` checks direct prerequisites; the ancestor check keeps a defective executor that recorded a later
/// operation's success before its prerequisite from making anything behind it eligible.
pub(crate) fn eligible_in(
    dag: &Dag<ReleaseOperationId>,
    state: &ReleaseExecutionStateV1,
    issued: &BTreeSet<ReleaseOperationId>,
) -> Vec<ReleaseOperationId> {
    let succeeded = |id: &ReleaseOperationId| state.operation_state(id).is_some_and(OperationState::is_success);
    dag.ready(
        |id| succeeded(id) || issued.contains(id),
        |id| state.operation_state(id) == Some(OperationState::Pending),
    )
    .filter_map(|ix| dag.key(ix).map(|id| (ix, id)))
    .filter(|(ix, id)| {
        dag.ancestors(&[*ix]).into_iter().all(|ancestor| {
            dag.key(ancestor).is_some_and(|prerequisite| {
                succeeded(prerequisite)
                    || (issued.contains(prerequisite) && is_registry_effect(id) && is_registry_effect(prerequisite))
            })
        })
    })
    .map(|(_, id)| id.clone())
    .collect()
}

/// [`eligible_in`] for a caller that has only the intent and issued nothing.
#[cfg(test)]
pub(crate) fn eligible_operations(
    intent: &ReleaseIntentV1,
    state: &ReleaseExecutionStateV1,
) -> Vec<ReleaseOperationId> {
    let dag = operation_dag(intent).expect("a test intent is a valid DAG");
    eligible_in(&dag, state, &BTreeSet::new())
}

#[cfg(test)]
mod tests {
    use callisto_model::{
        Ecosystem, ExecutionTrustProfileV1, OperationBlockReason, RegistryBindingDigest, RegistryBindingId,
        ReleaseInputSnapshotV1, ReleaseOperation, ReleasePackageId, SourceIdentity, Version,
    };

    use super::*;
    use crate::commands::release_test_support::{
        already_satisfied_event, attempt_event, confirmed_event, pending_state, publish_operation,
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

    /// The production tag provider behind the real executor, with an effect counter in place of the real publish.
    struct TagOnlyProviders<'a> {
        intent: ReleaseIntentV1,
        operation: crate::commands::release::provider::PreparedOperation,
        root: &'a std::path::Path,
        runner: &'a dyn callisto_model::CommandRunner,
        effects: std::cell::Cell<usize>,
    }

    impl TagOnlyProviders<'_> {
        fn request<'r>(
            &'r self,
            id: &'r ReleaseOperationId,
        ) -> crate::commands::release::provider::ProviderRequest<'r> {
            crate::commands::release::provider::ProviderRequest {
                id,
                operation: &self.operation,
                artifacts: None,
            }
        }
    }

    impl ReleaseProviderSet for TagOnlyProviders<'_> {
        fn intent(&self) -> &ReleaseIntentV1 {
            &self.intent
        }

        fn recheck_trust(&self) -> Result<(), GraphError> {
            Ok(())
        }

        fn preflight(
            &self,
            id: &ReleaseOperationId,
            _artifacts: Option<&VerifiedArtifactManifest<'_>>,
        ) -> Result<ReleasePreflight, GraphError> {
            use crate::commands::release::provider::{tag::TagProvider, ProviderContext, ReleaseProvider};
            let remote = crate::commands::release::optional_git_remote(self.root, self.runner)?;
            TagProvider.preflight(
                &ProviderContext::new(self.root, self.runner, remote.as_ref()),
                &self.request(id),
            )
        }

        fn observe(
            &self,
            _id: &ReleaseOperationId,
            _artifacts: Option<&VerifiedArtifactManifest<'_>>,
        ) -> Result<callisto_model::ProviderObservationV1, GraphError> {
            unreachable!("the executor preflights before it observes")
        }

        fn publish(
            &self,
            _permit: &ApplyPermit,
            _proof: &callisto_model::AbsentProof,
            _id: &ReleaseOperationId,
            _artifacts: Option<&VerifiedArtifactManifest<'_>>,
        ) -> Result<callisto_model::ExactEvidence, GraphError> {
            self.effects.set(self.effects.get() + 1);
            Err(GraphError::ReleaseInvariant {
                detail: "the effect must not run".to_owned(),
            })
        }
    }

    /// A git whose `rev-parse` fails fatally and whose remote reads as empty; it remembers each subcommand it ran.
    struct BrokenLocalGit(std::sync::Mutex<Vec<String>>);

    impl callisto_model::CommandRunner for BrokenLocalGit {
        fn run(
            &self,
            _program: &str,
            args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            let subcommand = args.first().copied().unwrap_or_default();
            self.0.lock().unwrap().push(subcommand.to_owned());
            let (exit_code, stdout, stderr) = match subcommand {
                "remote" => (0, "https://github.com/orin-dx/callisto", ""),
                "rev-parse" => (128, "", "fatal: not a git repository\n"),
                _ => (0, "", ""),
            };
            Ok(callisto_model::CommandOutput {
                exit_code: Some(exit_code),
                stdout: stdout.to_owned(),
                stderr: stderr.to_owned(),
            })
        }
    }

    /// A local tag lookup that fails fatally must stop the run at the observation: no tag is created, nothing is
    /// pushed and the effect is never issued.
    #[test]
    fn an_observation_that_errors_never_reaches_the_effect() {
        let package = ReleasePackageId::new(Ecosystem::Cargo, "demo").unwrap();
        let version = Version::semver(1, 0, 0);
        let tag = ReleaseOperation::tag(package.clone(), version.clone(), vec![]).unwrap();
        let intent = ReleaseIntentV1::new(
            callisto_model::ReleaseDecisionV1::new(vec![callisto_model::ReleaseDecisionEntry {
                package,
                target_version: version,
                reasons: vec![callisto_model::ReleaseInclusionReason::ExplicitSelection],
            }])
            .unwrap(),
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]).unwrap(),
            ExecutionTrustProfileV1::GitCommit,
            vec![tag.clone()],
            vec![],
        )
        .unwrap();
        let envelope = crate::commands::release_test_support::envelope(&intent);

        let dir = tempfile::tempdir().unwrap();
        let runner = BrokenLocalGit(std::sync::Mutex::new(Vec::new()));
        let providers = TagOnlyProviders {
            operation: crate::commands::release::provider::PreparedOperation::Tag(
                crate::commands::release::provider::TagOperation {
                    name: callisto_model::TagName::new_unchecked("demo@1.0.0".to_owned()),
                    target: callisto_model::CommitSha::parse(&"a".repeat(40)).unwrap(),
                    annotation: "demo@1.0.0".to_owned(),
                },
            ),
            intent,
            root: dir.path(),
            runner: &runner,
            effects: std::cell::Cell::new(0),
        };

        let result = execute_release(
            &providers,
            &callisto_model::ApplyPermit::force_for_tests(),
            &envelope,
            None,
        );

        assert!(
            matches!(
                result,
                Err(GraphError::ReleaseCommand {
                    failure: crate::error::CommandFailure::NonZeroExit { .. },
                    ..
                })
            ),
            "{result:?}"
        );
        assert_eq!(
            providers.effects.get(),
            0,
            "an observation that failed must not reach the effect"
        );
        assert_eq!(
            runner.0.lock().unwrap().as_slice(),
            ["remote", "rev-parse"],
            "no tag was created and nothing was pushed"
        );
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

    /// A registry publish may follow a registry publish that is only issued; anything announced may not.
    #[test]
    fn an_issued_registry_publish_unlocks_registry_publishes_but_not_the_tag() {
        let package = ReleasePackageId::new(Ecosystem::Npm, "a").unwrap();
        let dependent = ReleasePackageId::new(Ecosystem::Npm, "b").unwrap();
        let version = Version::semver(1, 0, 0);
        let registry = RegistryBindingId::new("npm", RegistryBindingDigest::from_normalized_binding(b"npm")).unwrap();
        let first =
            ReleaseOperation::registry_publish(package.clone(), version.clone(), registry.clone(), vec![]).unwrap();
        let second =
            ReleaseOperation::registry_publish(dependent.clone(), version.clone(), registry, vec![first.id().clone()])
                .unwrap();
        let tag = ReleaseOperation::tag(package.clone(), version.clone(), vec![first.id().clone()]).unwrap();
        let decision = callisto_model::ReleaseDecisionV1::new(vec![
            callisto_model::ReleaseDecisionEntry {
                package: package.clone(),
                target_version: version.clone(),
                reasons: vec![callisto_model::ReleaseInclusionReason::ExplicitSelection],
            },
            callisto_model::ReleaseDecisionEntry {
                package: dependent,
                target_version: version,
                reasons: vec![callisto_model::ReleaseInclusionReason::ExplicitSelection],
            },
        ])
        .unwrap();
        let intent = ReleaseIntentV1::new(
            decision,
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]).unwrap(),
            ExecutionTrustProfileV1::GitCommit,
            crate::commands::release::canonical_operation_order(
                [first.clone(), second.clone(), tag.clone()]
                    .into_iter()
                    .map(|operation| (operation.id().clone(), operation))
                    .collect(),
            )
            .unwrap(),
            vec![],
        )
        .unwrap();
        let dag = operation_dag(&intent).unwrap();
        let mut state = pending_state(&intent);
        state.apply(first.id(), &attempt_event()).unwrap();
        let mut issued = BTreeSet::new();
        issued.insert(first.id().clone());

        let eligible = eligible_in(&dag, &state, &issued);
        assert_eq!(
            eligible,
            [second.id().clone()],
            "only the dependent registry publish: {eligible:?}"
        );

        // Once the first is shown, the tag is eligible too.
        state.apply(first.id(), &confirmed_event(first.id())).unwrap();
        assert!(eligible_in(&dag, &state, &BTreeSet::new()).contains(tag.id()));
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
