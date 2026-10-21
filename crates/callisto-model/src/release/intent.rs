use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    ArtifactSlotId, CanonicalTranscript, ExecutionTrustProfileV1, IntentDigest, ReleaseDecisionV1,
    ReleaseInputSnapshotV1, ReleaseOperation, ReleaseOperationId, ReleaseOperationRole, SourceIdentity,
};

/// An immutable, canonical authorization intent. Graph owns its construction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "ReleaseIntentV1Wire", rename_all = "camelCase", deny_unknown_fields)]
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
#[derive(Deserialize, JsonSchema)]
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

impl TryFrom<ReleaseIntentV1Wire> for ReleaseIntentV1 {
    type Error = ReleaseIntentError;

    fn try_from(wire: ReleaseIntentV1Wire) -> Result<Self, Self::Error> {
        Self::from_wire(wire)
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

#[cfg(test)]
mod tests {
    use crate::Ecosystem;

    use super::super::test_support::{native_intent_with_slot, native_versions, registry_binding, test_intent};
    use super::*;
    use crate::release::*;

    #[test]
    fn durable_native_intents_preserve_operation_prerequisites_slots_and_digest() {
        for (ecosystem, raw) in native_versions() {
            let intent = native_intent_with_slot(ecosystem, raw);
            let serialized = serde_json::to_vec(&intent).unwrap();
            let decoded: ReleaseIntentV1 = serde_json::from_slice(&serialized).unwrap();
            assert_eq!(decoded, intent);
            assert_eq!(decoded.digest(), intent.digest());
            assert_eq!(serde_json::to_vec(&decoded).unwrap(), serialized);
            for operation in &decoded.operations {
                assert_eq!(operation.id.version.grammar(), ecosystem.version_grammar());
                assert_eq!(operation.id.version.render(), raw);
                for prerequisite in &operation.prerequisites {
                    assert_eq!(prerequisite.version.grammar(), ecosystem.version_grammar());
                }
            }
            assert_eq!(decoded.artifact_slots[0].version.render(), raw);
            assert_eq!(decoded.artifact_slots[0].version.grammar(), ecosystem.version_grammar());
        }
    }

    #[test]
    fn durable_prerequisites_decode_with_their_own_package_ecosystem() {
        let cargo = ReleaseOperation::tag(
            ReleasePackageId::parse("cargo/upstream").unwrap(),
            Version::semver(1, 2, 3),
            vec![],
        )
        .unwrap();
        let python = ReleaseOperation::tag(
            ReleasePackageId::parse("pypi/downstream").unwrap(),
            Version::parse_for_ecosystem("1.2.3", Ecosystem::Pypi).unwrap(),
            vec![cargo.id.clone()],
        )
        .unwrap();
        let intent = test_intent(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            vec![cargo, python],
        )
        .unwrap();
        let decoded: ReleaseIntentV1 = serde_json::from_slice(&serde_json::to_vec(&intent).unwrap()).unwrap();
        assert_eq!(decoded, intent);
        assert_eq!(decoded.operations[1].id.version.grammar(), VersionGrammar::Pep440);
        assert_eq!(
            decoded.operations[1].prerequisites[0].version.grammar(),
            VersionGrammar::SemVer
        );
    }

    #[test]
    fn native_decision_and_intent_validation_stays_strict() {
        let intent = native_intent_with_slot(Ecosystem::Pypi, "1.2.3");
        let decision = serde_json::to_value(&intent.decision).unwrap();
        for schema in ReleaseDecisionV1::READABLE_SCHEMA_VERSIONS {
            let mut tampered = decision.clone();
            tampered["schemaVersion"] = serde_json::json!(schema);
            tampered["entries"][0]["targetVersion"] = serde_json::json!("1.2.4");
            let error = serde_json::from_value::<ReleaseDecisionV1>(tampered).unwrap_err();
            assert!(error.to_string().contains("digest does not match"));
        }
        let mut unsupported = decision.clone();
        unsupported["schemaVersion"] = serde_json::json!(9);
        assert!(serde_json::from_value::<ReleaseDecisionV1>(unsupported)
            .unwrap_err()
            .to_string()
            .contains("unsupported release decision schema version 9"));
        let mut missing_reason = decision.clone();
        missing_reason["entries"][0]["reasons"] = serde_json::json!([]);
        assert!(serde_json::from_value::<ReleaseDecisionV1>(missing_reason).is_err());
        let mut duplicate = decision;
        let entry = duplicate["entries"][0].clone();
        duplicate["entries"].as_array_mut().unwrap().push(entry);
        assert!(serde_json::from_value::<ReleaseDecisionV1>(duplicate).is_err());

        let wire = serde_json::to_value(&intent).unwrap();
        let mut changed = wire.clone();
        changed["snapshot"]["source"]["sha"] = serde_json::json!("c".repeat(40));
        assert!(serde_json::from_value::<ReleaseIntentV1>(changed)
            .unwrap_err()
            .to_string()
            .contains("digest"));
        let mut changed = wire.clone();
        changed["operations"][0]["id"]["version"] = serde_json::json!("1.2.4");
        assert!(serde_json::from_value::<ReleaseIntentV1>(changed).is_err());
        let mut changed = wire.clone();
        changed["operations"][3]["id"]["role"]["slot"]["version"] = serde_json::json!("1.2.4");
        assert!(serde_json::from_value::<ReleaseIntentV1>(changed).is_err());
        let mut changed = wire.clone();
        changed["trustProfile"] = serde_json::json!("hermeticContent");
        assert!(serde_json::from_value::<ReleaseIntentV1>(changed).is_err());
        let mut changed = wire;
        changed["schemaVersion"] = serde_json::json!(9);
        assert!(serde_json::from_value::<ReleaseIntentV1>(changed).is_err());
    }

    #[test]
    fn registry_binding_is_credential_free_and_part_of_operation_identity() {
        let first = RegistryBindingId::new(
            "cratesIo",
            RegistryBindingDigest::from_normalized_binding("https://index.example.test/v1"),
        )
        .unwrap();
        let second = RegistryBindingId::new(
            "cratesIo",
            RegistryBindingDigest::from_normalized_binding("https://mirror.example.test/v1"),
        )
        .unwrap();
        assert_ne!(first, second);

        let wire = serde_json::to_string(&first).unwrap();
        assert!(wire.contains("registryKey"));
        assert!(wire.contains("bindingDigest"));
        assert!(!wire.contains("example.test"));
        assert!(!wire.contains("token"));

        let package = ReleasePackageId::parse("cargo/callisto-model").unwrap();
        let version = Version::semver(1, 2, 3);
        let first_operation = ReleaseOperationId::registry_publish(package.clone(), version.clone(), first);
        let second_operation = ReleaseOperationId::registry_publish(package, version, second);
        assert_ne!(first_operation, second_operation);
        assert_ne!(
            operation_id_text(&first_operation),
            operation_id_text(&second_operation)
        );
    }

    #[test]
    fn release_intent_rejects_noncanonical_duplicate_and_cyclic_dags() {
        let package = ReleasePackageId::parse("cargo/callisto-model").unwrap();
        let version = Version::semver(1, 2, 3);
        let publish =
            ReleaseOperation::registry_publish(package.clone(), version.clone(), registry_binding("cratesIo"), vec![])
                .unwrap();
        let tag = ReleaseOperation::tag(package, version, vec![publish.id().clone()]).unwrap();
        let snapshot = ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]);

        let reversed = test_intent(
            snapshot.clone(),
            ExecutionTrustProfileV1::GitCommit,
            vec![tag.clone(), publish.clone()],
        );
        assert!(matches!(reversed, Err(ReleaseIntentError::NonCanonicalOperationOrder)));

        let duplicate = test_intent(
            snapshot,
            ExecutionTrustProfileV1::GitCommit,
            vec![publish.clone(), publish],
        );
        assert!(matches!(duplicate, Err(ReleaseIntentError::DuplicateOperation { .. })));

        let mut cycle_publish = ReleaseOperation::registry_publish(
            ReleasePackageId::parse("cargo/callisto-model").unwrap(),
            Version::semver(1, 2, 3),
            registry_binding("cratesIo"),
            vec![tag.id().clone()],
        )
        .unwrap();
        // The public constructor permits referring to a distinct operation; intent validation owns cycle detection.
        cycle_publish.prerequisites = vec![tag.id().clone()];
        let cycle = test_intent(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("b".repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            vec![cycle_publish, tag],
        );
        assert!(matches!(cycle, Err(ReleaseIntentError::Cycle { .. })));
    }

    #[test]
    fn durable_intent_wire_rejects_forged_digest_unknown_fields_and_invalid_dag() {
        let operation = ReleaseOperation::registry_publish(
            ReleasePackageId::parse("cargo/callisto-model").unwrap(),
            Version::semver(1, 2, 3),
            registry_binding("cratesIo"),
            vec![],
        )
        .unwrap();
        let intent = test_intent(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("d".repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            vec![operation],
        )
        .unwrap();
        let value = serde_json::to_value(intent).unwrap();

        let mut forged_digest = value.clone();
        forged_digest["digest"] = serde_json::Value::String("0".repeat(64));
        assert!(serde_json::from_value::<ReleaseIntentV1>(forged_digest).is_err());

        let mut unknown = value.clone();
        unknown["unexpected"] = serde_json::Value::Bool(true);
        assert!(serde_json::from_value::<ReleaseIntentV1>(unknown).is_err());

        let mut bad_registry = value;
        bad_registry["operations"][0]["id"]["role"]["registryKey"] =
            serde_json::Value::String("https://token@example.test/registry".to_string());
        assert!(serde_json::from_value::<ReleaseIntentV1>(bad_registry).is_err());
    }
}
