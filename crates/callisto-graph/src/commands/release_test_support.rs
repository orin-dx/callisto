//! Release fixtures shared by this crate's unit tests.
//!
//! Every durable state now needs a validated run envelope and every state
//! change needs a proof token, so the tests build both here rather than in
//! each module.

use callisto_model::{
    ArtifactDigest, CommitSha, OperationEvent, ProviderEvidenceV1, ProviderObservationV1, ReleaseExecutionStateV1,
    ReleaseIntentV1, ReleaseOperationId, ReleaseOperationRole, ReleaseRunEnvelopeV1, ReleaseRunKindV1, TagName,
};

pub(crate) fn envelope_of_kind(intent: &ReleaseIntentV1, kind: ReleaseRunKindV1) -> ReleaseRunEnvelopeV1 {
    let orchestration = intent
        .artifact_slots
        .first()
        .map(|slot| slot.attestation_policy.workflow_commit.clone())
        .unwrap_or_else(|| CommitSha::parse(&"b".repeat(40)).unwrap());
    let manifest = (!intent.artifact_slots.is_empty()).then(|| ArtifactDigest::from_bytes(b"manifest"));
    ReleaseRunEnvelopeV1::new(kind, orchestration, intent, manifest).expect("fixture envelope matches its intent")
}

pub(crate) fn envelope(intent: &ReleaseIntentV1) -> ReleaseRunEnvelopeV1 {
    envelope_of_kind(intent, ReleaseRunKindV1::Initial)
}

pub(crate) fn pending_state(intent: &ReleaseIntentV1) -> ReleaseExecutionStateV1 {
    ReleaseExecutionStateV1::new(intent, envelope(intent)).expect("fixture state matches its intent")
}

pub(crate) fn recovery_state(intent: &ReleaseIntentV1) -> ReleaseExecutionStateV1 {
    ReleaseExecutionStateV1::new(intent, envelope_of_kind(intent, ReleaseRunKindV1::Recovery))
        .expect("fixture state matches its intent")
}

/// Role-matching evidence, as a real provider adapter would report it.
pub(crate) fn evidence_for(id: &ReleaseOperationId) -> ProviderEvidenceV1 {
    match &id.role {
        ReleaseOperationRole::RegistryPublish { .. } => ProviderEvidenceV1::RegistryVersion {
            version: id.version.clone(),
            checksum: None,
            yanked: None,
        },
        ReleaseOperationRole::Tag => ProviderEvidenceV1::GitTag {
            peeled_commit: CommitSha::parse(&"a".repeat(40)).unwrap(),
        },
        ReleaseOperationRole::ForgeRelease => ProviderEvidenceV1::ForgeRelease {
            tag_name: TagName::new_unchecked(format!("v{}", id.version.render())),
            draft: false,
        },
        ReleaseOperationRole::ArtifactUpload { .. } => ProviderEvidenceV1::ArtifactUpload {
            byte_length: 6,
            sha256: ArtifactDigest::from_bytes(b"binary"),
        },
    }
}

pub(crate) fn exact(id: &ReleaseOperationId) -> ProviderObservationV1 {
    ProviderObservationV1::Exact {
        evidence: evidence_for(id),
    }
}

pub(crate) fn attempt_event() -> OperationEvent {
    OperationEvent::Attempt {
        proof: ProviderObservationV1::Absent
            .absent_proof()
            .expect("absent mints a proof"),
    }
}

pub(crate) fn confirmed_event(id: &ReleaseOperationId) -> OperationEvent {
    OperationEvent::Confirmed {
        evidence: exact(id).exact_evidence().expect("exact mints evidence"),
    }
}

pub(crate) fn already_satisfied_event(id: &ReleaseOperationId) -> OperationEvent {
    OperationEvent::ObservedExactBeforeEffect {
        evidence: exact(id).exact_evidence().expect("exact mints evidence"),
    }
}

/// Drives one operation through the only legal path to `Published`.
pub(crate) fn publish_operation(state: &mut ReleaseExecutionStateV1, id: &ReleaseOperationId) {
    state.apply(id, &attempt_event()).unwrap();
    state.apply(id, &confirmed_event(id)).unwrap();
}
