use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use super::{
    ArtifactSlotId, CanonicalTranscript, ExecutionTrustProfileV1, IntentDigest, ReleaseDecisionV1,
    ReleaseInputSnapshotV1, ReleaseOperation, ReleaseOperationId, ReleaseOperationRole, SourceIdentity,
};

/// An immutable, canonical authorization intent. Graph owns its construction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseIntentV1 {
    pub schema_version: u8,
    pub decision: ReleaseDecisionV1,
    pub snapshot: ReleaseInputSnapshotV1,
    pub trust_profile: ExecutionTrustProfileV1,
    pub operations: Vec<ReleaseOperation>,
    pub artifact_slots: Vec<ArtifactSlotId>,
    pub(super) digest: IntentDigest,
}

/// The on-the-wire shape of [`ReleaseIntentV1`], before its invariants are checked.
///
/// Deserializing this (rather than `ReleaseIntentV1` itself) then calling
/// [`ReleaseIntentV1::from_wire`] keeps a caller's own typed [`ReleaseIntentError`]
/// intact -- deserializing straight into `ReleaseIntentV1` still validates, but folds a
/// validation failure into a generic `serde` string error, losing the code.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseIntentV1Wire {
    schema_version: u8,
    decision: ReleaseDecisionV1,
    snapshot: ReleaseInputSnapshotV1,
    trust_profile: ExecutionTrustProfileV1,
    operations: Vec<ReleaseOperation>,
    artifact_slots: Vec<ArtifactSlotId>,
    digest: IntentDigest,
}

impl ReleaseIntentV1 {
    /// Validates an already-parsed wire value into a well-formed intent.
    pub fn from_wire(wire: ReleaseIntentV1Wire) -> Result<Self, ReleaseIntentError> {
        if wire.schema_version != Self::SCHEMA_VERSION {
            return Err(ReleaseIntentError::UnsupportedSchemaVersion {
                type_name: "release intent",
                found: wire.schema_version,
                expected: Self::SCHEMA_VERSION,
            });
        }
        if !ReleaseDecisionV1::READABLE_SCHEMA_VERSIONS.contains(&wire.decision.schema_version) {
            return Err(ReleaseIntentError::UnsupportedSchemaVersion {
                type_name: "release intent decision",
                found: wire.decision.schema_version,
                expected: *ReleaseDecisionV1::READABLE_SCHEMA_VERSIONS.last().expect("nonempty"),
            });
        }
        if wire.snapshot.schema_version != ReleaseInputSnapshotV1::SCHEMA_VERSION {
            return Err(ReleaseIntentError::UnsupportedSchemaVersion {
                type_name: "release intent snapshot",
                found: wire.snapshot.schema_version,
                expected: ReleaseInputSnapshotV1::SCHEMA_VERSION,
            });
        }
        if wire.snapshot.packages.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(ReleaseIntentError::PackagesNotCanonical);
        }
        let intent = Self::new(
            wire.decision,
            wire.snapshot,
            wire.trust_profile,
            wire.operations,
            wire.artifact_slots,
        )?;
        if intent.digest != wire.digest {
            return Err(ReleaseIntentError::DigestMismatch);
        }
        Ok(intent)
    }
}

impl<'de> Deserialize<'de> for ReleaseIntentV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ReleaseIntentV1Wire::deserialize(deserializer)?;
        Self::from_wire(wire).map_err(serde::de::Error::custom)
    }
}

impl ReleaseIntentV1 {
    /// 3 adds the `forgePublish` role: publication is its own operation after
    /// every artifact upload, so a version-2 intent's DAG is not executable here.
    /// 4 adds the `platformPublish` role, which an earlier reader cannot execute.
    /// 5 removes the release `profile`.
    pub const SCHEMA_VERSION: u8 = 5;

    pub fn new(
        decision: ReleaseDecisionV1,
        snapshot: ReleaseInputSnapshotV1,
        trust_profile: ExecutionTrustProfileV1,
        operations: Vec<ReleaseOperation>,
        mut artifact_slots: Vec<ArtifactSlotId>,
    ) -> Result<Self, ReleaseIntentError> {
        if !matches!(trust_profile, ExecutionTrustProfileV1::GitCommit)
            || !matches!(snapshot.source, SourceIdentity::GitCommit { .. })
        {
            return Err(ReleaseIntentError::UnsupportedTrustProfile);
        }
        validate_operations(&operations)?;
        validate_operation_roster(&decision, &operations)?;
        artifact_slots.sort();
        if artifact_slots.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ReleaseIntentError::DuplicateArtifactSlot);
        }
        if let Some(slot) = artifact_slots.iter().find(|slot| {
            !decision
                .entries
                .iter()
                .any(|entry| entry.package == slot.package && entry.target_version == slot.version)
        }) {
            return Err(ReleaseIntentError::ArtifactSlotOutsideDecision {
                package: slot.package.to_string(),
                version: slot.version.to_string(),
                asset: slot.asset_name.clone(),
            });
        }
        validate_artifact_upload_roster(&operations, &artifact_slots)?;
        let digest = digest_intent(&decision, &snapshot, trust_profile, &operations, &artifact_slots);
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            decision,
            snapshot,
            trust_profile,
            operations,
            artifact_slots,
            digest,
        })
    }

    pub fn digest(&self) -> &IntentDigest {
        &self.digest
    }
}

fn digest_intent(
    decision: &ReleaseDecisionV1,
    snapshot: &ReleaseInputSnapshotV1,
    trust_profile: ExecutionTrustProfileV1,
    operations: &[ReleaseOperation],
    artifact_slots: &[ArtifactSlotId],
) -> IntentDigest {
    let mut transcript = CanonicalTranscript::intent_v1();
    transcript.push_bytes("schema", [ReleaseIntentV1::SCHEMA_VERSION]);
    transcript.push_str("decision", decision.digest.as_str());
    transcript.push_str("snapshot", snapshot.digest().as_str());
    transcript.push_str(
        "trust-profile",
        match trust_profile {
            ExecutionTrustProfileV1::GitCommit => "git-commit",
            ExecutionTrustProfileV1::HermeticContent => "hermetic-content",
        },
    );
    for operation in operations {
        transcript.push_str("operation", &operation_id_text(&operation.id));
        for prerequisite in &operation.prerequisites {
            transcript.push_str("prerequisite", &operation_id_text(prerequisite));
        }
    }
    for slot in artifact_slots {
        transcript.push_str("artifact-slot.package", &slot.package.to_string());
        transcript.push_str("artifact-slot.version", slot.version.render());
        transcript.push_str("artifact-slot.platform", &slot.platform);
        transcript.push_str("artifact-slot.asset", &slot.asset_name);
        transcript.push_str(
            "artifact-slot.repository",
            &slot.attestation_policy.repository.as_slug(),
        );
        transcript.push_str("artifact-slot.workflow-path", &slot.attestation_policy.workflow_path);
        transcript.push_str(
            "artifact-slot.workflow-commit",
            slot.attestation_policy.workflow_commit.as_str(),
        );
    }
    IntentDigest::from_transcript(&transcript)
}

fn validate_operation_roster(
    decision: &ReleaseDecisionV1,
    operations: &[ReleaseOperation],
) -> Result<(), ReleaseIntentError> {
    let roster = &decision.entries;
    for operation in operations {
        if !roster
            .iter()
            .any(|entry| entry.package == operation.id.package && entry.target_version == operation.id.version)
        {
            return Err(ReleaseIntentError::OperationOutsideDecision {
                id: Box::new(operation.id.clone()),
            });
        }
    }
    Ok(())
}

fn validate_artifact_upload_roster(
    operations: &[ReleaseOperation],
    artifact_slots: &[ArtifactSlotId],
) -> Result<(), ReleaseIntentError> {
    let mut uploads = operations
        .iter()
        .filter_map(|operation| operation.id.role.artifact_slot())
        .collect::<Vec<_>>();
    uploads.sort();
    if uploads.iter().copied().ne(artifact_slots.iter()) {
        return Err(ReleaseIntentError::MismatchedArtifactUploadRoster);
    }
    Ok(())
}

pub(super) fn operation_id_text(id: &ReleaseOperationId) -> String {
    let role = match &id.role {
        ReleaseOperationRole::RegistryPublish { registry } => format!(
            "publish:{}:{}",
            registry.registry_key().as_str(),
            registry.binding_digest().as_str()
        ),
        ReleaseOperationRole::Tag => "tag".to_string(),
        ReleaseOperationRole::ForgeRelease => "forge-release".to_string(),
        ReleaseOperationRole::ArtifactUpload { slot } => format!(
            "artifact-upload:{}:{}:{}:{}:{}:{}",
            slot.platform,
            slot.asset_name,
            slot.attestation_policy.repository,
            slot.attestation_policy.workflow_path,
            slot.attestation_policy.workflow_commit.as_str(),
            slot.version.render(),
        ),
        ReleaseOperationRole::ForgePublish => "forge-publish".to_string(),
        ReleaseOperationRole::PlatformPublish { registry, platform } => format!(
            "platform-publish:{}:{}:{}:{}",
            registry.registry_key().as_str(),
            registry.binding_digest().as_str(),
            platform.name(),
            platform.directory(),
        ),
    };
    format!("{}|{}|{}", id.package, role, id.version.render())
}

fn validate_operations(operations: &[ReleaseOperation]) -> Result<(), ReleaseIntentError> {
    let mut ids = BTreeSet::new();
    for operation in operations {
        if !ids.insert(operation.id.clone()) {
            return Err(ReleaseIntentError::DuplicateOperation {
                id: Box::new(operation.id.clone()),
            });
        }
    }
    for operation in operations {
        if operation.prerequisites.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(ReleaseIntentError::NonCanonicalPrerequisiteOrder {
                id: Box::new(operation.id.clone()),
            });
        }
        for prerequisite in &operation.prerequisites {
            if prerequisite == &operation.id {
                return Err(ReleaseIntentError::SelfPrerequisite {
                    id: Box::new(operation.id.clone()),
                });
            }
            if !ids.contains(prerequisite) {
                return Err(ReleaseIntentError::UnknownPrerequisite {
                    id: Box::new(operation.id.clone()),
                    prerequisite: Box::new(prerequisite.clone()),
                });
            }
        }
    }
    let edges: BTreeMap<_, _> = operations
        .iter()
        .map(|operation| (operation.id.clone(), operation.prerequisites.clone()))
        .collect();
    let canonical = stable_kahn_order(&edges)?;
    if operations.iter().map(ReleaseOperation::id).ne(canonical.iter()) {
        return Err(ReleaseIntentError::NonCanonicalOperationOrder);
    }
    Ok(())
}

fn stable_kahn_order(
    prerequisites: &BTreeMap<ReleaseOperationId, Vec<ReleaseOperationId>>,
) -> Result<Vec<ReleaseOperationId>, ReleaseIntentError> {
    let mut remaining: BTreeMap<_, usize> = prerequisites
        .iter()
        .map(|(id, prerequisites)| (id.clone(), prerequisites.len()))
        .collect();
    let mut dependents = BTreeMap::<ReleaseOperationId, Vec<ReleaseOperationId>>::new();
    for (id, prerequisites) in prerequisites {
        for prerequisite in prerequisites {
            dependents.entry(prerequisite.clone()).or_default().push(id.clone());
        }
    }
    let mut ready: BTreeSet<_> = remaining
        .iter()
        .filter_map(|(id, count)| (*count == 0).then_some(id.clone()))
        .collect();
    let mut ordered = Vec::with_capacity(prerequisites.len());
    while let Some(id) = ready.pop_first() {
        ordered.push(id.clone());
        for dependent in dependents.get(&id).into_iter().flatten() {
            let count = remaining.get_mut(dependent).expect("known DAG node");
            *count -= 1;
            if *count == 0 {
                ready.insert(dependent.clone());
            }
        }
    }
    if ordered.len() != prerequisites.len() {
        let id = remaining
            .into_iter()
            .find_map(|(id, count)| (count != 0).then_some(id))
            .expect("a nonempty cyclic DAG has a remaining node");
        return Err(ReleaseIntentError::Cycle { id: Box::new(id) });
    }
    Ok(ordered)
}

/// Validation failure for a durable intent DAG.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, miette::Diagnostic)]
#[non_exhaustive]
pub enum ReleaseIntentError {
    #[error("durable release intents support only clean Git commit trust in v1")]
    #[diagnostic(
        code(E284),
        help("build the intent from a clean Git commit source with the GitCommit trust profile")
    )]
    UnsupportedTrustProfile,
    #[error("release operation `{id:?}` is not authorized by the embedded release decision")]
    #[diagnostic(
        code(E285),
        help("only include operations for packages present in the release decision")
    )]
    OperationOutsideDecision { id: Box<ReleaseOperationId> },
    #[error("release intent repeats an artifact slot")]
    #[diagnostic(code(E286), help("list each artifact slot at most once"))]
    DuplicateArtifactSlot,
    #[error(
        "asset `{asset}` is built by `{package}@{version}`, which is not in this release; \
         release it too, or put it in the product's [[fixed-group]]"
    )]
    #[diagnostic(
        code(E287),
        help("release the asset's package too, or put it in the product's [[fixed-group]]")
    )]
    ArtifactSlotOutsideDecision {
        package: String,
        version: String,
        asset: String,
    },
    #[error("artifact upload operations must exactly match the declared artifact slots")]
    #[diagnostic(
        code(E288),
        help("make each artifact slot correspond to exactly one artifact upload operation")
    )]
    MismatchedArtifactUploadRoster,
    #[error("release operations are not in canonical order")]
    #[diagnostic(code(E289), help("re-derive the intent instead of hand-editing operation order"))]
    NonCanonicalOperationOrder,
    #[error("duplicate release operation `{id:?}`")]
    #[diagnostic(code(E290), help("list each release operation at most once"))]
    DuplicateOperation { id: Box<ReleaseOperationId> },
    #[error("prerequisites for `{id:?}` are not in canonical order")]
    #[diagnostic(code(E291), help("re-derive the intent instead of hand-editing prerequisite order"))]
    NonCanonicalPrerequisiteOrder { id: Box<ReleaseOperationId> },
    #[error("release operation `{id:?}` requires unknown operation `{prerequisite:?}`")]
    #[diagnostic(
        code(E292),
        help("only reference prerequisites that are also in the intent's operation list")
    )]
    UnknownPrerequisite {
        id: Box<ReleaseOperationId>,
        prerequisite: Box<ReleaseOperationId>,
    },
    #[error("release operation `{id:?}` requires itself")]
    #[diagnostic(code(E293), help("remove the operation from its own prerequisite list"))]
    SelfPrerequisite { id: Box<ReleaseOperationId> },
    #[error("release operation DAG contains a cycle at `{id:?}`")]
    #[diagnostic(code(E294), help("break the prerequisite cycle among release operations"))]
    Cycle { id: Box<ReleaseOperationId> },
    #[error("unsupported {type_name} schema version {found}; this build reads version {expected}")]
    #[diagnostic(
        code(E295),
        help(
            "re-derive this file with the current build; a release-intent wire shape is never reused across versions"
        )
    )]
    UnsupportedSchemaVersion {
        type_name: &'static str,
        found: u8,
        expected: u8,
    },
    #[error("release input packages are not canonical")]
    #[diagnostic(code(E296), help("re-derive the intent instead of hand-editing package order"))]
    PackagesNotCanonical,
    #[error("release intent digest does not match canonical content")]
    #[diagnostic(code(E297), help("re-derive the intent instead of hand-editing its fields"))]
    DigestMismatch,
}
