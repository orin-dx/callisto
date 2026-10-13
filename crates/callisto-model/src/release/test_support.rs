use crate::release_observation::ProviderEvidenceV1;
use crate::release_observation::ProviderObservationV1;
use crate::release_transition::OperationEvent;
use crate::{CommitSha, Ecosystem};

use crate::release::*;

pub(super) fn test_envelope(intent: &ReleaseIntentV1) -> ReleaseRunEnvelopeV1 {
    let orchestration = intent
        .artifact_slots
        .first()
        .map(|slot| slot.attestation_policy.workflow_commit.clone())
        .unwrap_or_else(|| CommitSha::parse(&"b".repeat(40)).unwrap());
    let manifest = (!intent.artifact_slots.is_empty()).then(|| ArtifactDigest::from_bytes(b"manifest"));
    ReleaseRunEnvelopeV1::new(orchestration, intent, manifest).unwrap()
}

pub(super) fn pending_state(intent: &ReleaseIntentV1) -> ReleaseExecutionStateV1 {
    ReleaseExecutionStateV1::new(intent, test_envelope(intent)).unwrap()
}

/// Role-matching evidence for one operation, as a real provider adapter
/// would report it.
pub(super) fn evidence_for(id: &ReleaseOperationId) -> ProviderEvidenceV1 {
    match &id.role {
        ReleaseOperationRole::RegistryPublish { .. } | ReleaseOperationRole::PlatformPublish { .. } => {
            ProviderEvidenceV1::RegistryVersion {
                version: id.version.clone(),
                checksum: None,
                yanked: None,
            }
        }
        ReleaseOperationRole::Tag => ProviderEvidenceV1::GitTag {
            peeled_commit: CommitSha::parse(&"a".repeat(40)).unwrap(),
        },
        ReleaseOperationRole::ForgeRelease | ReleaseOperationRole::ForgePublish => ProviderEvidenceV1::ForgeRelease {
            tag_name: crate::TagName::new_unchecked(format!("v{}", id.version.render())),
            draft: false,
        },
        ReleaseOperationRole::ArtifactUpload { .. } => ProviderEvidenceV1::ArtifactUpload {
            byte_length: 6,
            sha256: ArtifactDigest::from_bytes(b"binary"),
        },
    }
}

pub(super) fn exact_observation(id: &ReleaseOperationId) -> ProviderObservationV1 {
    ProviderObservationV1::Exact {
        evidence: evidence_for(id),
    }
}

pub(super) fn attempt_event() -> OperationEvent {
    OperationEvent::Attempt {
        proof: ProviderObservationV1::Absent
            .absent_proof()
            .expect("absent mints a proof"),
    }
}

pub(super) fn confirmed_event(id: &ReleaseOperationId) -> OperationEvent {
    OperationEvent::Confirmed {
        evidence: exact_observation(id).exact_evidence().expect("exact mints evidence"),
    }
}

/// Drives one operation through the only legal path to `Published`.
pub(super) fn publish_operation(state: &mut ReleaseExecutionStateV1, id: &ReleaseOperationId) {
    state.apply(id, &attempt_event()).unwrap();
    state.apply(id, &confirmed_event(id)).unwrap();
}

pub(super) fn registry_binding(key: &str) -> RegistryBindingId {
    RegistryBindingId::new(
        key,
        RegistryBindingDigest::from_normalized_binding(format!("https://{key}.example.test/index")),
    )
    .unwrap()
}

pub(super) fn native_versions() -> [(Ecosystem, &'static str); 8] {
    [
        (Ecosystem::Pypi, "1.2.3"),
        (Ecosystem::Pypi, "1.2.3rc1"),
        (Ecosystem::Pypi, "1!2.3.post1.dev2"),
        (Ecosystem::Pypi, "1.2"),
        (Ecosystem::Cargo, "1.2.3"),
        (Ecosystem::Cargo, "1.2.3-beta.1+build.2"),
        (Ecosystem::Npm, "1.2.3"),
        (Ecosystem::Npm, "1.2.3-beta.1+build.2"),
    ]
}

pub(super) fn native_intent_with_slot(ecosystem: Ecosystem, raw: &str) -> ReleaseIntentV1 {
    let package = ReleasePackageId::new(ecosystem, "example").unwrap();
    let version = Version::parse_for_ecosystem(raw, ecosystem).unwrap();
    let slot = ArtifactSlotId::new(
        package.clone(),
        version.clone(),
        "linux-x64",
        "example.tar.gz",
        GitHubRepository::parse("owner/repo").unwrap(),
        RELEASE_COORDINATOR_WORKFLOW_PATH,
        CommitSha::parse(&"b".repeat(40)).unwrap(),
    )
    .unwrap();
    let mut operations = Vec::new();
    let mut prerequisites = Vec::new();
    if ecosystem == Ecosystem::Npm {
        let platform = ReleaseOperation::new(
            ReleaseOperationId::platform_publish(
                package.clone(),
                version.clone(),
                registry_binding("native"),
                PlatformPackageV1::new(ReleasePackageId::parse("npm/example-linux").unwrap(), "platform/linux")
                    .unwrap(),
            ),
            vec![],
        )
        .unwrap();
        prerequisites.push(platform.id.clone());
        operations.push(platform);
    }
    let publish = ReleaseOperation::registry_publish(
        package.clone(),
        version.clone(),
        registry_binding("native"),
        prerequisites,
    )
    .unwrap();
    let tag = ReleaseOperation::tag(package.clone(), version.clone(), vec![publish.id().clone()]).unwrap();
    let forge = ReleaseOperation::forge_release(package.clone(), version.clone(), vec![tag.id().clone()]).unwrap();
    let upload = ReleaseOperation::artifact_upload(slot.clone(), vec![forge.id().clone()]).unwrap();
    let complete = ReleaseOperation::forge_publish(package, version, vec![upload.id().clone()]).unwrap();
    operations.extend([publish, tag, forge, upload, complete]);
    test_intent_with_slots(
        ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]),
        ExecutionTrustProfileV1::GitCommit,
        operations,
        vec![slot],
    )
    .unwrap()
}

pub(super) fn test_intent(
    snapshot: Result<ReleaseInputSnapshotV1, ReleaseInputSnapshotError>,
    trust_profile: ExecutionTrustProfileV1,
    operations: Vec<ReleaseOperation>,
) -> Result<ReleaseIntentV1, ReleaseIntentError> {
    test_intent_with_slots(snapshot, trust_profile, operations, vec![])
}

pub(super) fn test_intent_with_slots(
    snapshot: Result<ReleaseInputSnapshotV1, ReleaseInputSnapshotError>,
    trust_profile: ExecutionTrustProfileV1,
    operations: Vec<ReleaseOperation>,
    slots: Vec<ArtifactSlotId>,
) -> Result<ReleaseIntentV1, ReleaseIntentError> {
    let mut entries = Vec::new();
    for operation in &operations {
        if !entries.iter().any(|entry: &ReleaseDecisionEntry| {
            entry.package == operation.id.package && entry.target_version == operation.id.version
        }) {
            entries.push(ReleaseDecisionEntry {
                package: operation.id.package.clone(),
                target_version: operation.id.version.clone(),
                reasons: vec![ReleaseInclusionReason::ExplicitSelection],
            });
        }
    }
    ReleaseIntentV1::new(
        ReleaseDecisionV1::new(entries).expect("test operations define a roster"),
        snapshot.expect("test snapshot is valid"),
        trust_profile,
        operations,
        slots,
    )
}
