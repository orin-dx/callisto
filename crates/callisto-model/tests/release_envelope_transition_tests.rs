//! Structural invariants of the run envelope and the operation transition table.

use callisto_model::*;

fn sha(c: char) -> CommitSha {
    CommitSha::parse(&c.to_string().repeat(40)).unwrap()
}

fn tag_intent() -> ReleaseIntentV1 {
    let package = ReleasePackageId::parse("cargo/demo").unwrap();
    let version = Version::semver(1, 2, 3);
    ReleaseIntentV1::new(
        ReleaseDecisionV1::new(vec![ReleaseDecisionEntry {
            package: package.clone(),
            target_version: version.clone(),
            reasons: vec![ReleaseInclusionReason::ExplicitSelection],
        }])
        .unwrap(),
        ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]).unwrap(),
        ExecutionTrustProfileV1::GitCommit,
        vec![ReleaseOperation::tag(package, version, vec![]).unwrap()],
        vec![],
    )
    .unwrap()
}

/// An intent with one compiled-binary slot attested by coordinator `b`.
fn slotted_intent() -> (ReleaseIntentV1, ArtifactSlotId) {
    let package = ReleasePackageId::parse("cargo/demo").unwrap();
    let version = Version::semver(1, 2, 3);
    let slot = ArtifactSlotId::new(
        package.clone(),
        version.clone(),
        "x86_64-unknown-linux-gnu",
        "demo.tar.gz",
        GitHubRepository::parse("orin-dx/callisto").unwrap(),
        callisto_model::RELEASE_COORDINATOR_WORKFLOW_PATH,
        sha('b'),
    )
    .unwrap();
    let intent = ReleaseIntentV1::new(
        ReleaseDecisionV1::new(vec![ReleaseDecisionEntry {
            package,
            target_version: version,
            reasons: vec![ReleaseInclusionReason::ExplicitSelection],
        }])
        .unwrap(),
        ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]).unwrap(),
        ExecutionTrustProfileV1::GitCommit,
        vec![ReleaseOperation::artifact_upload(slot.clone(), vec![]).unwrap()],
        vec![slot.clone()],
    )
    .unwrap();
    (intent, slot)
}

fn absent_proof() -> AbsentProof {
    ProviderObservationV1::Absent.absent_proof().unwrap()
}

fn tag_evidence() -> ExactEvidence {
    ProviderObservationV1::Exact {
        evidence: ProviderEvidenceV1::GitTag {
            peeled_commit: sha('a'),
        },
    }
    .exact_evidence()
    .unwrap()
}

fn event_of(kind: OperationEventKind) -> OperationEvent {
    match kind {
        OperationEventKind::Attempt => OperationEvent::Attempt { proof: absent_proof() },
        OperationEventKind::ObservedExactBeforeEffect => OperationEvent::ObservedExactBeforeEffect {
            evidence: tag_evidence(),
        },
        OperationEventKind::Confirmed => OperationEvent::Confirmed {
            evidence: tag_evidence(),
        },
        OperationEventKind::EffectFailedAndAbsent => OperationEvent::EffectFailedAndAbsent { proof: absent_proof() },
        OperationEventKind::Blocked => OperationEvent::Blocked {
            reason: OperationBlockReason::IndeterminateAttempt,
        },
    }
}

const STATES: [OperationState; 6] = [
    OperationState::Pending,
    OperationState::Attempting,
    OperationState::Published,
    OperationState::AlreadySatisfied,
    OperationState::Failed,
    OperationState::Blocked {
        reason: OperationBlockReason::IndeterminateAttempt,
    },
];

const EVENTS: [OperationEventKind; 5] = [
    OperationEventKind::Attempt,
    OperationEventKind::ObservedExactBeforeEffect,
    OperationEventKind::Confirmed,
    OperationEventKind::EffectFailedAndAbsent,
    OperationEventKind::Blocked,
];

/// The whole legal edge set, written out independently of the implementation.
fn allowed(from: OperationState, event: OperationEventKind) -> Option<OperationState> {
    use OperationEventKind as E;
    use OperationState as S;
    match (from, event) {
        (S::Pending, E::Attempt) => Some(S::Attempting),
        (S::Pending, E::ObservedExactBeforeEffect) => Some(S::AlreadySatisfied),
        (S::Attempting, E::Confirmed) => Some(S::Published),
        (S::Attempting, E::EffectFailedAndAbsent) => Some(S::Failed),
        (S::Pending | S::Attempting, E::Blocked) => Some(S::Blocked {
            reason: OperationBlockReason::IndeterminateAttempt,
        }),
        _ => None,
    }
}

#[test]
fn every_state_event_pair_matches_the_explicit_allowed_list() {
    for from in STATES {
        for event in EVENTS {
            let result = transition(from, &event_of(event));
            match allowed(from, event) {
                Some(expected) => assert_eq!(
                    result,
                    Ok(expected),
                    "({from:?}, {event:?}) must be allowed and reach {expected:?}"
                ),
                None => assert!(
                    result.is_err(),
                    "({from:?}, {event:?}) must be rejected, got {result:?}"
                ),
            }
        }
    }
}

#[test]
fn no_bounded_event_sequence_reaches_success_without_a_legal_edge() {
    let mut frontier = vec![(OperationState::Pending, Vec::new())];
    for _ in 0..4 {
        let mut next = Vec::new();
        for (state, path) in &frontier {
            for event in EVENTS {
                let Ok(reached) = transition(*state, &event_of(event)) else {
                    continue;
                };
                let mut path = path.clone();
                path.push(event);
                assert_eq!(
                    reached == OperationState::Attempting,
                    event == OperationEventKind::Attempt,
                    "Attempting is reachable only through an Attempt carrying an AbsentProof: {path:?}"
                );
                if reached == OperationState::Published {
                    assert_eq!(
                        *state,
                        OperationState::Attempting,
                        "Published is reachable only from Attempting: {path:?}"
                    );
                    assert_eq!(
                        path.first(),
                        Some(&OperationEventKind::Attempt),
                        "a published operation must have attempted first: {path:?}"
                    );
                }
                next.push((reached, path));
            }
        }
        for (state, path) in &frontier {
            if matches!(
                state,
                OperationState::Published
                    | OperationState::AlreadySatisfied
                    | OperationState::Failed
                    | OperationState::Blocked { .. }
            ) {
                assert!(
                    EVENTS.iter().all(|e| transition(*state, &event_of(*e)).is_err()),
                    "terminal state {state:?} must absorb every event: {path:?}"
                );
            }
        }
        frontier = next;
    }
}

#[test]
fn envelope_rejects_each_cross_field_mismatch() {
    let intent = tag_intent();
    let envelope = ReleaseRunEnvelopeV1::new(sha('b'), &intent, None).unwrap();
    assert!(envelope.validate_for_intent(&intent).is_ok());

    // A slot-less intent must carry no manifest digest.
    assert!(matches!(
        ReleaseRunEnvelopeV1::new(sha('b'), &intent, Some(ArtifactDigest::from_bytes(b"manifest"))),
        Err(ReleaseRunEnvelopeError::UnexpectedArtifactManifest)
    ));

    let (slotted, _slot) = slotted_intent();
    assert!(matches!(
        ReleaseRunEnvelopeV1::new(sha('b'), &slotted, None),
        Err(ReleaseRunEnvelopeError::MissingArtifactManifest)
    ));
    // The coordinator revision must be the one that attested the slots.
    assert!(matches!(
        ReleaseRunEnvelopeV1::new(sha('c'), &slotted, Some(ArtifactDigest::from_bytes(b"manifest"))),
        Err(ReleaseRunEnvelopeError::MismatchedOrchestrationRevision)
    ));
    assert!(ReleaseRunEnvelopeV1::new(sha('b'), &slotted, Some(ArtifactDigest::from_bytes(b"manifest"))).is_ok());

    // Intent digest and release source are read out of the intent,
    // so they can only disagree if forged on the wire.
    for (field, value, expected) in [
        (
            "intentDigest",
            serde_json::json!("0".repeat(64)),
            ReleaseRunEnvelopeError::MismatchedIntent,
        ),
        (
            "releaseSourceRevision",
            serde_json::json!("c".repeat(40)),
            ReleaseRunEnvelopeError::MismatchedReleaseSource,
        ),
    ] {
        let mut wire = serde_json::to_value(&envelope).unwrap();
        wire[field] = value;
        let forged: ReleaseRunEnvelopeV1 = serde_json::from_value(wire).unwrap();
        assert_eq!(
            forged.validate_for_intent(&intent),
            Err(expected),
            "a forged `{field}` must be rejected"
        );
    }
}

#[test]
fn observation_rejects_evidence_that_does_not_match_the_operation_role() {
    let intent = tag_intent();
    let tag = intent.operations[0].id().clone();
    assert!(ReleaseOperationObservationV1::new(
        tag.clone(),
        ProviderObservationV1::Exact {
            evidence: ProviderEvidenceV1::GitTag {
                peeled_commit: sha('a')
            },
        },
    )
    .is_ok());
    assert!(matches!(
        ReleaseOperationObservationV1::new(
            tag,
            ProviderObservationV1::Exact {
                evidence: ProviderEvidenceV1::ForgeRelease {
                    tag_name: TagName::parse("v1.2.3").unwrap(),
                    draft: false,
                },
            },
        ),
        Err(ProviderObservationError::EvidenceRoleMismatch { .. })
    ));
}

#[test]
fn an_operation_can_never_be_marked_attempting_without_observing_absence() {
    let intent = tag_intent();
    let envelope = ReleaseRunEnvelopeV1::new(sha('b'), &intent, None).unwrap();
    let mut state = ReleaseExecutionStateV1::new(&intent, envelope).unwrap();
    let operation = intent.operations[0].id().clone();

    // The only constructor of an AbsentProof is an Absent observation.
    assert!(ProviderObservationV1::Exact {
        evidence: ProviderEvidenceV1::GitTag {
            peeled_commit: sha('a')
        },
    }
    .absent_proof()
    .is_none());
    assert!(ProviderObservationV1::Absent.exact_evidence().is_none());

    assert_eq!(
        state
            .apply(&operation, &OperationEvent::Attempt { proof: absent_proof() })
            .unwrap(),
        OperationState::Attempting
    );
    assert!(matches!(
        state.apply(&operation, &OperationEvent::Attempt { proof: absent_proof() }),
        Err(ReleaseStateError::InvalidTransition { .. })
    ));
}
