use std::cmp::Ordering;
use std::collections::BTreeSet;

use super::*;
use crate::identity::assert_string_wire_schema;
use crate::release_observation::{ProviderEvidenceV1, ProviderObservationV1};
use crate::release_transition::OperationEvent;
use crate::{CommitSha, Ecosystem, PackageId};

#[test]
fn release_package_id_schema_describes_canonical_strings() {
    for raw in ["cargo/example", "npm/@scope/example", "pypi/example-package"] {
        let id = ReleasePackageId::parse(raw).unwrap();
        assert_string_wire_schema(&id, "ReleasePackageId", raw);
    }
}

#[test]
fn github_repository_schema_describes_the_normalized_slug() {
    let repository = GitHubRepository::parse("Owner/Repository.git").unwrap();
    assert_string_wire_schema(&repository, "GitHubRepository", "owner/repository");
}

#[test]
fn release_digest_schemas_keep_distinct_names_for_string_values() {
    let raw = "a".repeat(64);
    assert_string_wire_schema(&ArtifactDigest::parse(&raw).unwrap(), "ArtifactDigest", &raw);
    assert_string_wire_schema(&DecisionDigest::parse(&raw).unwrap(), "DecisionDigest", &raw);
    assert_string_wire_schema(&SemanticInputDigest::parse(&raw).unwrap(), "SemanticInputDigest", &raw);
    assert_string_wire_schema(&IntentDigest::parse(&raw).unwrap(), "IntentDigest", &raw);
    assert_string_wire_schema(
        &RegistryBindingDigest::parse(&raw).unwrap(),
        "RegistryBindingDigest",
        &raw,
    );
}

#[test]
fn string_leaf_definitions_stay_distinct_in_a_shared_schema() {
    let mut generator = schemars::gen::SchemaGenerator::default();
    let references = [
        ("PackageId", generator.subschema_for::<PackageId>()),
        ("CommitSha", generator.subschema_for::<CommitSha>()),
        ("Version", generator.subschema_for::<Version>()),
        ("VersionReq", generator.subschema_for::<crate::VersionReq>()),
        ("ReleasePackageId", generator.subschema_for::<ReleasePackageId>()),
        ("GitHubRepository", generator.subschema_for::<GitHubRepository>()),
        ("ArtifactDigest", generator.subschema_for::<ArtifactDigest>()),
        ("DecisionDigest", generator.subschema_for::<DecisionDigest>()),
        ("SemanticInputDigest", generator.subschema_for::<SemanticInputDigest>()),
        ("IntentDigest", generator.subschema_for::<IntentDigest>()),
        (
            "RegistryBindingDigest",
            generator.subschema_for::<RegistryBindingDigest>(),
        ),
    ];
    assert_eq!(generator.definitions().len(), references.len());
    for (name, reference) in references {
        let reference = serde_json::to_value(reference).unwrap();
        assert_eq!(reference["$ref"], format!("#/definitions/{name}"));
        let definition = serde_json::to_value(&generator.definitions()[name]).unwrap();
        assert_eq!(definition["type"], "string");
    }
}

fn test_envelope(intent: &ReleaseIntentV1) -> ReleaseRunEnvelopeV1 {
    let orchestration = intent
        .artifact_slots
        .first()
        .map(|slot| slot.attestation_policy.workflow_commit.clone())
        .unwrap_or_else(|| CommitSha::parse(&"b".repeat(40)).unwrap());
    let manifest = (!intent.artifact_slots.is_empty()).then(|| ArtifactDigest::from_bytes(b"manifest"));
    ReleaseRunEnvelopeV1::new(orchestration, intent, manifest).unwrap()
}

fn pending_state(intent: &ReleaseIntentV1) -> ReleaseExecutionStateV1 {
    ReleaseExecutionStateV1::new(intent, test_envelope(intent)).unwrap()
}

/// Role-matching evidence for one operation, as a real provider adapter
/// would report it.
fn evidence_for(id: &ReleaseOperationId) -> ProviderEvidenceV1 {
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

fn exact_observation(id: &ReleaseOperationId) -> ProviderObservationV1 {
    ProviderObservationV1::Exact {
        evidence: evidence_for(id),
    }
}

fn attempt_event() -> OperationEvent {
    OperationEvent::Attempt {
        proof: ProviderObservationV1::Absent
            .absent_proof()
            .expect("absent mints a proof"),
    }
}

fn confirmed_event(id: &ReleaseOperationId) -> OperationEvent {
    OperationEvent::Confirmed {
        evidence: exact_observation(id).exact_evidence().expect("exact mints evidence"),
    }
}

/// Drives one operation through the only legal path to `Published`.
fn publish_operation(state: &mut ReleaseExecutionStateV1, id: &ReleaseOperationId) {
    state.apply(id, &attempt_event()).unwrap();
    state.apply(id, &confirmed_event(id)).unwrap();
}

/// `rename_all` on an internally-tagged enum only renames the `kind`
/// discriminant, never fields inside a variant -- the exact gap that
/// shipped `ReleasePrActionV1`'s snake_case `pull_request_number` to
/// production (see `.changeset/fix-release-pr-action-camel-case.md`).
/// `ReleaseInclusionReason` had the identical gap.
#[test]
fn inclusion_reason_variant_fields_serialize_as_camel_case() {
    let fixed = ReleaseInclusionReason::FixedGroup {
        group_id: "workspace".to_string(),
    };
    let value = serde_json::to_value(&fixed).unwrap();
    assert_eq!(value["kind"], "fixedGroup");
    assert_eq!(value["groupId"], "workspace");
    assert!(value.get("group_id").is_none());

    let linked = ReleaseInclusionReason::LinkedGroup {
        group_id: "demo".to_string(),
    };
    let value = serde_json::to_value(&linked).unwrap();
    assert_eq!(value["kind"], "linkedGroup");
    assert_eq!(value["groupId"], "demo");

    let cascade = ReleaseInclusionReason::Cascade {
        from: ReleasePackageId::new(Ecosystem::Cargo, "upstream").unwrap(),
        edge_kind: "peer".to_string(),
    };
    let value = serde_json::to_value(&cascade).unwrap();
    assert_eq!(value["kind"], "cascade");
    assert_eq!(value["edgeKind"], "peer");
    assert!(value.get("edge_kind").is_none());

    let policy = ReleaseInclusionReason::PreReleasePolicy {
        policy_id: "beta".to_string(),
    };
    let value = serde_json::to_value(&policy).unwrap();
    assert_eq!(value["kind"], "preReleasePolicy");
    assert_eq!(value["policyId"], "beta");
}

fn registry_binding(key: &str) -> RegistryBindingId {
    RegistryBindingId::new(
        key,
        RegistryBindingDigest::from_normalized_binding(format!("https://{key}.example.test/index")),
    )
    .unwrap()
}

fn native_versions() -> [(Ecosystem, &'static str); 8] {
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

fn native_intent_with_slot(ecosystem: Ecosystem, raw: &str) -> ReleaseIntentV1 {
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

#[test]
fn durable_decisions_decode_native_versions_in_both_readable_schemas() {
    for (ecosystem, raw) in native_versions() {
        let decision = ReleaseDecisionV1::new(vec![ReleaseDecisionEntry {
            package: ReleasePackageId::new(ecosystem, "example").unwrap(),
            target_version: Version::parse_for_ecosystem(raw, ecosystem).unwrap(),
            reasons: vec![ReleaseInclusionReason::Cascade {
                from: ReleasePackageId::parse("npm/upstream").unwrap(),
                edge_kind: "runtime".to_string(),
            }],
        }])
        .unwrap();
        let serialized = serde_json::to_value(&decision).unwrap();
        assert_eq!(serialized["entries"][0]["targetVersion"], raw);
        for schema in ReleaseDecisionV1::READABLE_SCHEMA_VERSIONS {
            let mut wire = serialized.clone();
            wire["schemaVersion"] = serde_json::json!(schema);
            let decoded: ReleaseDecisionV1 = serde_json::from_value(wire).unwrap();
            assert_eq!(decoded, decision);
            assert_eq!(serde_json::to_value(decoded).unwrap(), serialized);
        }
    }
}

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
fn durable_native_artifact_manifests_preserve_slots_and_exact_digest_bytes() {
    for (ecosystem, raw) in native_versions() {
        let intent = native_intent_with_slot(ecosystem, raw);
        let slot = intent.artifact_slots[0].clone();
        let digest = ArtifactDigest::from_bytes(b"binary");
        let manifest = ArtifactManifestV1::new(
            &intent,
            vec![ArtifactManifestEntryV1 {
                attestation: GitHubArtifactAttestationV1 {
                    repository: slot.attestation_policy.repository.clone(),
                    workflow_path: slot.attestation_policy.workflow_path.clone(),
                    workflow_commit: slot.attestation_policy.workflow_commit.clone(),
                    subject_digest: digest.clone(),
                    source_commit: slot.attestation_policy.workflow_commit.clone(),
                },
                slot,
                digest,
                byte_length: 6,
            }],
        )
        .unwrap();
        let serialized = serde_json::to_vec(&manifest).unwrap();
        let decoded: ArtifactManifestV1 = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(decoded, manifest);
        decoded.validate_for_intent(&intent).unwrap();
        assert_eq!(decoded.digest(), manifest.digest());
        assert_eq!(serde_json::to_vec(&decoded).unwrap(), serialized);
    }
}

#[test]
fn durable_native_receipts_preserve_bound_registry_evidence_and_roster() {
    for (ecosystem, raw) in native_versions() {
        let intent = native_intent_with_slot(ecosystem, raw);
        let mut state = pending_state(&intent);
        for operation in &intent.operations {
            publish_operation(&mut state, operation.id());
        }
        let receipt = ReleaseReceiptV1::from_state(&intent, &state).unwrap();
        let serialized = serde_json::to_vec(&receipt).unwrap();
        let decoded: ReleaseReceiptV1 = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(decoded, receipt);
        decoded.validate_for_intent(&intent).unwrap();
        assert_eq!(serde_json::to_vec(&decoded).unwrap(), serialized);
        let observation = decoded.observation(intent.operations[0].id()).unwrap();
        let ProviderObservationV1::Exact {
            evidence: ProviderEvidenceV1::RegistryVersion { version, .. },
        } = observation
        else {
            panic!("registry receipt must carry its native version");
        };
        assert_eq!(version.grammar(), ecosystem.version_grammar());
        assert_eq!(version.render(), raw);
    }
}

#[test]
fn contextual_release_decoders_reject_cross_grammar_and_non_string_versions() {
    for (ecosystem, raw) in [
        (Ecosystem::Cargo, "1!2.3rc1"),
        (Ecosystem::Npm, "1.2.3a1"),
        (Ecosystem::Pypi, "1.2.3-alpha.beta"),
    ] {
        let intent = native_intent_with_slot(ecosystem, "1.2.3");
        let mut entry = serde_json::to_value(&intent.decision.entries[0]).unwrap();
        let mut operation = serde_json::to_value(&intent.operations[0].id).unwrap();
        let mut slot = serde_json::to_value(&intent.artifact_slots[0]).unwrap();
        for invalid in [serde_json::json!(raw), serde_json::json!(123), serde_json::Value::Null] {
            entry["targetVersion"] = invalid.clone();
            operation["version"] = invalid.clone();
            slot["version"] = invalid;
            assert!(serde_json::from_value::<ReleaseDecisionEntry>(entry.clone()).is_err());
            assert!(serde_json::from_value::<ReleaseOperationId>(operation.clone()).is_err());
            assert!(serde_json::from_value::<ArtifactSlotId>(slot.clone()).is_err());
        }
    }
}

#[test]
fn contextual_release_decoders_reject_an_unimplemented_version_grammar() {
    let intent = native_intent_with_slot(Ecosystem::Pypi, "1.2.3");
    let mut entry = serde_json::to_value(&intent.decision.entries[0]).unwrap();
    let mut operation = serde_json::to_value(&intent.operations[0].id).unwrap();
    let mut slot = serde_json::to_value(&intent.artifact_slots[0]).unwrap();
    entry["package"] = serde_json::json!("maven/example");
    operation["package"] = serde_json::json!("maven/example");
    slot["package"] = serde_json::json!("maven/example");
    for error in [
        serde_json::from_value::<ReleaseDecisionEntry>(entry).unwrap_err(),
        serde_json::from_value::<ReleaseOperationId>(operation).unwrap_err(),
        serde_json::from_value::<ArtifactSlotId>(slot).unwrap_err(),
    ] {
        assert!(error.to_string().contains("no versioning implementation"));
    }
}

#[test]
fn native_receipts_reject_tampered_evidence_and_unsupported_schema() {
    let intent = native_intent_with_slot(Ecosystem::Pypi, "1.2.3");
    let mut state = pending_state(&intent);
    for operation in &intent.operations {
        publish_operation(&mut state, operation.id());
    }
    let receipt = ReleaseReceiptV1::from_state(&intent, &state).unwrap();
    let wire = serde_json::to_value(&receipt).unwrap();
    let registry_index = wire["observations"]
        .as_array()
        .unwrap()
        .iter()
        .position(|entry| entry["observation"]["evidence"]["kind"] == "registryVersion")
        .unwrap();
    let mut changed = wire.clone();
    changed["observations"][registry_index]["observation"]["evidence"]["version"] = serde_json::json!("1.2.4");
    assert!(serde_json::from_value::<ReleaseReceiptV1>(changed)
        .unwrap_err()
        .to_string()
        .contains("provider version does not match"));
    let mut changed = wire.clone();
    changed["schemaVersion"] = serde_json::json!(9);
    assert!(serde_json::from_value::<ReleaseReceiptV1>(changed).is_err());
    let mut changed = wire.clone();
    let repeated = changed["observations"][registry_index].clone();
    changed["observations"].as_array_mut().unwrap().push(repeated);
    assert!(serde_json::from_value::<ReleaseReceiptV1>(changed).is_err());
    let mut changed = wire;
    changed["intentDigest"] = serde_json::json!("f".repeat(64));
    let mismatched: ReleaseReceiptV1 = serde_json::from_value(changed).unwrap();
    assert!(matches!(
        mismatched.validate_for_intent(&intent),
        Err(ReleaseReceiptError::MismatchedIntent)
    ));
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
fn contextual_release_wire_keeps_unknown_and_duplicate_field_rejection() {
    let intent = native_intent_with_slot(Ecosystem::Pypi, "1.2.3");
    let mut entry = serde_json::to_value(&intent.decision.entries[0]).unwrap();
    entry["extra"] = serde_json::json!(true);
    assert!(serde_json::from_value::<ReleaseDecisionEntry>(entry).is_err());
    let mut operation = serde_json::to_value(&intent.operations[0].id).unwrap();
    operation["extra"] = serde_json::json!(true);
    assert!(serde_json::from_value::<ReleaseOperationId>(operation).is_err());
    let mut slot = serde_json::to_value(&intent.artifact_slots[0]).unwrap();
    slot["extra"] = serde_json::json!(true);
    assert!(serde_json::from_value::<ArtifactSlotId>(slot).is_err());
    assert!(serde_json::from_str::<ReleaseDecisionEntry>(
            r#"{"package":"pypi/example","targetVersion":"1.2.3","targetVersion":"1.2.3","reasons":[{"kind":"explicitSelection"}]}"#
        )
        .is_err());
    let operation = serde_json::to_string(&intent.operations[0].id).unwrap();
    let duplicate = operation.replacen('{', "{\"version\":\"1.2.3\",", 1);
    assert!(serde_json::from_str::<ReleaseOperationId>(&duplicate).is_err());
    let slot = serde_json::to_string(&intent.artifact_slots[0]).unwrap();
    let duplicate = slot.replacen('{', "{\"version\":\"1.2.3\",", 1);
    assert!(serde_json::from_str::<ArtifactSlotId>(&duplicate).is_err());
}

fn test_intent(
    snapshot: Result<ReleaseInputSnapshotV1, ReleaseInputSnapshotError>,
    trust_profile: ExecutionTrustProfileV1,
    operations: Vec<ReleaseOperation>,
) -> Result<ReleaseIntentV1, ReleaseIntentError> {
    test_intent_with_slots(snapshot, trust_profile, operations, vec![])
}

fn test_intent_with_slots(
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

#[test]
fn release_package_id_requires_a_qualified_canonical_identity() {
    assert_eq!(
        ReleasePackageId::parse("cargo/shared").unwrap().to_string(),
        "cargo/shared"
    );
    assert_eq!(ReleasePackageId::parse("npm/shared").unwrap().to_string(), "npm/shared");
    assert!(ReleasePackageId::parse("shared").is_err());
    assert!(ReleasePackageId::parse("cargo:shared").is_err());
    assert!(ReleasePackageId::parse("unknown/shared").is_err());
    assert!(ReleasePackageId::parse("cargo/../shared").is_err());
    assert!(ReleasePackageId::new(Ecosystem::Cargo, "").is_err());
}

#[test]
fn release_package_ids_do_not_conflate_ecosystems() {
    let cargo = ReleasePackageId::parse("cargo/shared").unwrap();
    let npm = ReleasePackageId::parse("npm/shared").unwrap();
    assert_ne!(cargo, npm);
    assert!(cargo < npm);
}

#[test]
fn release_package_id_deserialization_rejects_a_bare_identity() {
    let parsed = serde_json::from_str::<ReleasePackageId>(r#""shared""#);
    assert!(parsed.is_err());
}

#[test]
fn github_repository_parses_valid_owner_repo_and_renders_slug() {
    let repo = GitHubRepository::parse("orin-dx/callisto").unwrap();
    assert_eq!(repo.owner(), "orin-dx");
    assert_eq!(repo.repo(), "callisto");
    assert_eq!(repo.as_slug(), "orin-dx/callisto");
    assert_eq!(repo.to_string(), "orin-dx/callisto");
}

#[test]
fn github_repository_rejects_embedded_whitespace_extra_parts_and_leading_trailing_hyphen() {
    // The prior `is_safe_github_repository` accepted this (a bare
    // `split_once('/')` with no character-class check at all) -- this is
    // the exact behavior-change gap the audit found.
    assert!(GitHubRepository::parse("owner name/repo").is_err());
    assert!(GitHubRepository::parse("owner/repo name").is_err());
    assert!(GitHubRepository::parse("owner").is_err());
    assert!(GitHubRepository::parse("owner/repo/extra").is_err());
    assert!(GitHubRepository::parse("-owner/repo").is_err());
    assert!(GitHubRepository::parse("owner-/repo").is_err());
    assert!(GitHubRepository::parse("owner/-repo").is_err());
    assert!(GitHubRepository::parse("owner/repo-").is_err());
    assert!(GitHubRepository::parse("/repo").is_err());
    assert!(GitHubRepository::parse("owner/").is_err());
}

#[test]
fn github_repository_serializes_and_deserializes_as_its_slug_string() {
    let repo = GitHubRepository::parse("orin-dx/callisto").unwrap();
    let json = serde_json::to_string(&repo).unwrap();
    assert_eq!(json, "\"orin-dx/callisto\"");
    let round_tripped: GitHubRepository = serde_json::from_str(&json).unwrap();
    assert_eq!(round_tripped, repo);
    assert!(serde_json::from_str::<GitHubRepository>("\"owner name/repo\"").is_err());
}

#[test]
fn decision_rejects_divergent_target_versions_within_a_fixed_group() {
    let a = ReleasePackageId::parse("cargo/crate-a").unwrap();
    let b = ReleasePackageId::parse("cargo/crate-b").unwrap();
    let entries = vec![
        ReleaseDecisionEntry {
            package: a,
            target_version: Version::semver(1, 1, 0),
            reasons: vec![ReleaseInclusionReason::FixedGroup {
                group_id: "demo".to_string(),
            }],
        },
        ReleaseDecisionEntry {
            package: b,
            target_version: Version::semver(1, 2, 0),
            reasons: vec![ReleaseInclusionReason::FixedGroup {
                group_id: "demo".to_string(),
            }],
        },
    ];
    assert!(matches!(
        ReleaseDecisionV1::new(entries),
        Err(ReleaseDecisionError::DivergentGroupTarget { group_id, .. }) if group_id == "demo"
    ));
}

#[test]
fn decision_rejects_divergent_target_versions_within_a_linked_group() {
    let a = ReleasePackageId::parse("cargo/crate-a").unwrap();
    let b = ReleasePackageId::parse("npm/crate-b").unwrap();
    let entries = vec![
        ReleaseDecisionEntry {
            package: a,
            target_version: Version::semver(2, 0, 0),
            reasons: vec![ReleaseInclusionReason::LinkedGroup {
                group_id: "linked-demo".to_string(),
            }],
        },
        ReleaseDecisionEntry {
            package: b,
            target_version: Version::semver(2, 0, 1),
            reasons: vec![ReleaseInclusionReason::LinkedGroup {
                group_id: "linked-demo".to_string(),
            }],
        },
    ];
    assert!(matches!(
        ReleaseDecisionV1::new(entries),
        Err(ReleaseDecisionError::DivergentGroupTarget { group_id, .. }) if group_id == "linked-demo"
    ));
}

#[test]
fn decision_accepts_agreeing_target_versions_within_a_fixed_group() {
    let a = ReleasePackageId::parse("cargo/crate-a").unwrap();
    let b = ReleasePackageId::parse("cargo/crate-b").unwrap();
    let entries = vec![
        ReleaseDecisionEntry {
            package: a,
            target_version: Version::semver(1, 1, 0),
            reasons: vec![ReleaseInclusionReason::FixedGroup {
                group_id: "demo".to_string(),
            }],
        },
        ReleaseDecisionEntry {
            package: b,
            target_version: Version::semver(1, 1, 0),
            reasons: vec![ReleaseInclusionReason::FixedGroup {
                group_id: "demo".to_string(),
            }],
        },
    ];
    assert!(ReleaseDecisionV1::new(entries).is_ok());
}

/// The digest-validating `Deserialize` impl calls `new()` internally, so
/// a hand-crafted (but internally digest-consistent) decision claiming
/// divergent group targets must be rejected the same way a freshly
/// derived one is -- this is the path a merge-commit verifier reading a
/// committed decision file actually exercises, not just direct
/// `ReleaseDecisionV1::new()` calls from a version plan.
#[test]
fn decision_deserialization_rejects_divergent_group_targets_even_with_a_self_consistent_digest() {
    let a = ReleasePackageId::parse("cargo/crate-a").unwrap();
    let b = ReleasePackageId::parse("cargo/crate-b").unwrap();
    let entries = vec![
        ReleaseDecisionEntry {
            package: a,
            target_version: Version::semver(1, 1, 0),
            reasons: vec![ReleaseInclusionReason::FixedGroup {
                group_id: "demo".to_string(),
            }],
        },
        ReleaseDecisionEntry {
            package: b,
            target_version: Version::semver(1, 2, 0),
            reasons: vec![ReleaseInclusionReason::FixedGroup {
                group_id: "demo".to_string(),
            }],
        },
    ];
    // Hand-compute a digest matching this exact (invalid) entry set, so
    // the failure below is provably the group-divergence check and not
    // just the pre-existing digest-mismatch guard.
    let digest = decision_digest(&entries);
    let wire = serde_json::json!({
        "schemaVersion": ReleaseDecisionV1::SCHEMA_VERSION,
        "entries": entries,
        "digest": digest,
    });
    let result: Result<ReleaseDecisionV1, _> = serde_json::from_value(wire);
    assert!(
        result.is_err(),
        "expected divergent group targets to be rejected on deserialize"
    );
}

#[test]
fn unreleased_version_reason_is_decision_schema_two() {
    assert_eq!(ReleaseDecisionV1::SCHEMA_VERSION, 2);
    assert_eq!(
        serde_json::to_value(ReleaseInclusionReason::UnreleasedVersion).unwrap(),
        serde_json::json!({ "kind": "unreleasedVersion" })
    );
    let decision = ReleaseDecisionV1::new(vec![ReleaseDecisionEntry {
        package: ReleasePackageId::new(Ecosystem::Cargo, "demo").unwrap(),
        target_version: Version::semver(1, 0, 0),
        reasons: vec![ReleaseInclusionReason::UnreleasedVersion],
    }])
    .unwrap();
    let mut wire = serde_json::to_value(&decision).unwrap();
    assert_eq!(
        serde_json::from_value::<ReleaseDecisionV1>(wire.clone()).unwrap(),
        decision
    );
    wire["schemaVersion"] = serde_json::json!(3);
    let error = serde_json::from_value::<ReleaseDecisionV1>(wire)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("unsupported release decision schema version 3"),
        "{error}"
    );
}

#[test]
fn committed_v1_decision_file_still_reads_and_writes_as_v2() {
    let raw = include_str!("../../tests/fixtures/release-decision-v1-0.8.0.json");
    let value: serde_json::Value = serde_json::from_str(raw).unwrap();
    assert_eq!(value["schemaVersion"], 1);
    let decision: ReleaseDecisionV1 = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(decision.entries.len(), value["entries"].as_array().unwrap().len());
    assert_eq!(decision.digest.to_string(), value["digest"].as_str().unwrap());
    assert_eq!(serde_json::to_value(&decision).unwrap()["schemaVersion"], 2);
}

#[test]
fn digests_are_distinct_validated_lowercase_hex_newtypes() {
    let hex = "a".repeat(64);
    assert_eq!(IntentDigest::parse(&hex).unwrap().as_str(), hex);
    assert!(IntentDigest::parse(&"A".repeat(64)).is_err());
    assert!(ArtifactDigest::parse("abcd").is_err());
    assert!(serde_json::from_str::<SemanticInputDigest>(r#""ABC""#).is_err());
}

#[test]
fn artifact_digest_hashes_exact_bytes() {
    assert_eq!(
        ArtifactDigest::from_bytes(b"callisto").to_string(),
        "04d52bfb8ce8b5a37e6a15b8c002419d2555543855cb4a7972ca80b2d8eadbf0"
    );
    assert_ne!(
        ArtifactDigest::from_bytes(b"callisto"),
        ArtifactDigest::from_bytes(b"callisto\n")
    );
}

#[test]
fn artifact_manifest_requires_exact_slot_digest_and_source_binding() {
    let package = ReleasePackageId::parse("cargo/demo").unwrap();
    let version = Version::semver(1, 0, 0);
    let slot = ArtifactSlotId::new(
        package.clone(),
        version.clone(),
        "x86_64-unknown-linux-gnu",
        "demo.tar.gz",
        GitHubRepository::parse("orin-dx/callisto").unwrap(),
        ".github/workflows/release.yml",
        CommitSha::parse(&"b".repeat(40)).unwrap(),
    )
    .unwrap();
    let operation = ReleaseOperation::artifact_upload(slot.clone(), vec![]).unwrap();
    let intent = test_intent_with_slots(
        ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]),
        ExecutionTrustProfileV1::GitCommit,
        vec![operation],
        vec![slot.clone()],
    )
    .unwrap();
    let digest = ArtifactDigest::from_bytes(b"binary");
    let manifest = ArtifactManifestV1::new(
        &intent,
        vec![ArtifactManifestEntryV1 {
            slot,
            digest: digest.clone(),
            byte_length: 6,
            attestation: GitHubArtifactAttestationV1 {
                repository: GitHubRepository::parse("orin-dx/callisto").unwrap(),
                workflow_path: ".github/workflows/release.yml".to_string(),
                workflow_commit: CommitSha::parse(&"b".repeat(40)).unwrap(),
                subject_digest: digest,
                source_commit: CommitSha::parse(&"b".repeat(40)).unwrap(),
            },
        }],
    )
    .unwrap();
    manifest.validate_for_intent(&intent).unwrap();
}

/// Top-level `source_commit` pins to the release source (the
/// intent's immutable snapshot), never the orchestration revision that
/// attested the binary -- a recovery rerun deliberately builds an older
/// release commit with current coordinator workflow code, so these two
/// facts differ and must not be conflated.
#[test]
fn artifact_manifest_source_commit_is_release_source_not_orchestration_revision() {
    let package = ReleasePackageId::parse("cargo/demo").unwrap();
    let version = Version::semver(1, 0, 0);
    let release_source = CommitSha::parse(&"a".repeat(40)).unwrap();
    let orchestration_revision = CommitSha::parse(&"c".repeat(40)).unwrap();
    assert_ne!(
        release_source, orchestration_revision,
        "fixture must exercise diverging commits"
    );
    let slot = ArtifactSlotId::new(
        package,
        version,
        "x86_64-unknown-linux-gnu",
        "demo.tar.gz",
        GitHubRepository::parse("orin-dx/callisto").unwrap(),
        ".github/workflows/release.yml",
        orchestration_revision.clone(),
    )
    .unwrap();
    let operation = ReleaseOperation::artifact_upload(slot.clone(), vec![]).unwrap();
    let intent = test_intent_with_slots(
        ReleaseInputSnapshotV1::new(
            SourceIdentity::GitCommit {
                sha: release_source.clone(),
            },
            vec![],
        ),
        ExecutionTrustProfileV1::GitCommit,
        vec![operation],
        vec![slot.clone()],
    )
    .unwrap();
    let digest = ArtifactDigest::from_bytes(b"binary");
    let manifest = ArtifactManifestV1::new(
        &intent,
        vec![ArtifactManifestEntryV1 {
            slot,
            digest: digest.clone(),
            byte_length: 6,
            attestation: GitHubArtifactAttestationV1 {
                repository: GitHubRepository::parse("orin-dx/callisto").unwrap(),
                workflow_path: ".github/workflows/release.yml".to_string(),
                workflow_commit: orchestration_revision.clone(),
                subject_digest: digest,
                source_commit: orchestration_revision.clone(),
            },
        }],
    )
    .unwrap();
    assert_eq!(manifest.source_commit, release_source);
    assert_ne!(manifest.source_commit, orchestration_revision);
}

/// An entry whose `attestation.source_commit` is set to the
/// release source instead of the orchestration revision (when the two
/// differ) is rejected -- otherwise a recovery rerun's manifest would
/// silently bind GitHub's attested source digest to the wrong commit.
#[test]
fn artifact_manifest_rejects_attestation_source_commit_swapped_for_release_source() {
    let package = ReleasePackageId::parse("cargo/demo").unwrap();
    let version = Version::semver(1, 0, 0);
    let release_source = CommitSha::parse(&"a".repeat(40)).unwrap();
    let orchestration_revision = CommitSha::parse(&"c".repeat(40)).unwrap();
    assert_ne!(
        release_source, orchestration_revision,
        "fixture must exercise diverging commits"
    );
    let slot = ArtifactSlotId::new(
        package,
        version,
        "x86_64-unknown-linux-gnu",
        "demo.tar.gz",
        GitHubRepository::parse("orin-dx/callisto").unwrap(),
        ".github/workflows/release.yml",
        orchestration_revision.clone(),
    )
    .unwrap();
    let operation = ReleaseOperation::artifact_upload(slot.clone(), vec![]).unwrap();
    let intent = test_intent_with_slots(
        ReleaseInputSnapshotV1::new(
            SourceIdentity::GitCommit {
                sha: release_source.clone(),
            },
            vec![],
        ),
        ExecutionTrustProfileV1::GitCommit,
        vec![operation],
        vec![slot.clone()],
    )
    .unwrap();
    let digest = ArtifactDigest::from_bytes(b"binary");
    let result = ArtifactManifestV1::new(
        &intent,
        vec![ArtifactManifestEntryV1 {
            slot,
            digest: digest.clone(),
            byte_length: 6,
            attestation: GitHubArtifactAttestationV1 {
                repository: GitHubRepository::parse("orin-dx/callisto").unwrap(),
                workflow_path: ".github/workflows/release.yml".to_string(),
                workflow_commit: orchestration_revision,
                subject_digest: digest,
                // Swapped: release source instead of the orchestration revision.
                source_commit: release_source,
            },
        }],
    );
    assert!(matches!(result, Err(ArtifactManifestError::MismatchedAttestation)));
}

/// Two `ArtifactUpload` operations that share package/version/platform/asset_name
/// but differ only in `attestation_policy` are distinct `ArtifactSlotId`s and must
/// remain distinct `ReleaseOperationId`s under `Ord`, not just `Eq` -- otherwise
/// `BTreeSet`/`BTreeMap` keys silently collapse them (the `a.cmp(b) == Equal implies
/// a == b` invariant those collections require).
#[test]
fn artifact_upload_ids_differing_only_by_attestation_policy_stay_distinct_under_ord() {
    let package = ReleasePackageId::parse("cargo/demo").unwrap();
    let version = Version::semver(1, 0, 0);
    let slot_a = ArtifactSlotId::new(
        package.clone(),
        version.clone(),
        "x86_64-unknown-linux-gnu",
        "demo.tar.gz",
        GitHubRepository::parse("orin-dx/callisto").unwrap(),
        ".github/workflows/release.yml",
        CommitSha::parse(&"a".repeat(40)).unwrap(),
    )
    .unwrap();
    let slot_b = ArtifactSlotId::new(
        package,
        version,
        "x86_64-unknown-linux-gnu",
        "demo.tar.gz",
        GitHubRepository::parse("orin-dx/callisto").unwrap(),
        ".github/workflows/release.yml",
        CommitSha::parse(&"b".repeat(40)).unwrap(),
    )
    .unwrap();
    assert_ne!(slot_a, slot_b, "the two slots must differ only by attestation_policy");

    let id_a = ReleaseOperationId::artifact_upload(slot_a.clone());
    let id_b = ReleaseOperationId::artifact_upload(slot_b.clone());

    assert_ne!(id_a, id_b, "distinct attestation policies must stay Eq-distinct");
    assert_ne!(
        id_a.cmp(&id_b),
        Ordering::Equal,
        "distinct attestation policies must not compare Ord::Equal"
    );

    let mut ids = BTreeSet::new();
    assert!(ids.insert(id_a.clone()), "first id must insert");
    assert!(
        ids.insert(id_b.clone()),
        "second id must NOT be displaced by the first in a BTreeSet"
    );
    assert_eq!(ids.len(), 2);

    // Exercise the real ReleaseIntentV1::new / validate_operations path: this must
    // NOT be rejected as a duplicate operation.
    let op_a = ReleaseOperation::artifact_upload(slot_a.clone(), vec![]).unwrap();
    let op_b = ReleaseOperation::artifact_upload(slot_b.clone(), vec![]).unwrap();
    let (first, second) = if op_a.id() <= op_b.id() {
        (op_a, op_b)
    } else {
        (op_b, op_a)
    };
    let (first_slot, second_slot) = if first.id() == &id_a {
        (slot_a, slot_b)
    } else {
        (slot_b, slot_a)
    };
    let intent = test_intent_with_slots(
        ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("d".repeat(40)).unwrap(), vec![]),
        ExecutionTrustProfileV1::GitCommit,
        vec![first, second],
        vec![first_slot, second_slot],
    );
    assert!(
        intent.is_ok(),
        "two artifact uploads differing only by attestation_policy must not collide: {intent:?}"
    );
}

/// Residual instance of the same defect class fixed above for
/// `attestation_policy`: `ReleaseOperationId::cmp` used to key on
/// `version.render()` alone, dropping `version.grammar()`. The same
/// literal string parses under more than one grammar -- `"1.2.3"` is
/// valid both as SemVer and PEP 440 -- so two `ReleaseOperationId`s
/// that are Eq-distinct only by `Version::grammar` collapsed to
/// `Ordering::Equal` under the old `render()`-only `Ord`, violating
/// the invariant `BTreeSet`/`BTreeMap` require: `a.cmp(b) == Equal`
/// iff `a == b`.
#[test]
fn operation_ids_differing_only_by_version_grammar_stay_distinct_under_ord() {
    let package = ReleasePackageId::parse("cargo/demo").unwrap();
    let v_semver = Version::parse("1.2.3", VersionGrammar::SemVer).unwrap();
    let v_pep440 = Version::parse("1.2.3", VersionGrammar::Pep440).unwrap();

    // Precondition: the identical literal renders identically under both
    // grammars, but the parsed Versions are NOT the same value -- this is
    // exactly the collision `version_ord_key` must resolve.
    assert_eq!(v_semver.render(), v_pep440.render());
    assert_ne!(
        v_semver, v_pep440,
        "SemVer and PEP 440 parses of the identical literal must remain Eq-distinct"
    );

    let id_a = ReleaseOperationId::tag(package.clone(), v_semver);
    let id_b = ReleaseOperationId::tag(package, v_pep440);

    assert_ne!(id_a, id_b, "distinct version grammars must stay Eq-distinct");
    assert_ne!(
        id_a.cmp(&id_b),
        Ordering::Equal,
        "distinct version grammars must not compare Ord::Equal"
    );

    let mut ids = BTreeSet::new();
    assert!(ids.insert(id_a.clone()), "first id must insert");
    assert!(
        ids.insert(id_b.clone()),
        "second id must NOT be displaced by the first in a BTreeSet"
    );
    assert_eq!(ids.len(), 2, "BTreeSet must not silently collapse distinct-grammar ids");

    // Sanity check the ordinary case still behaves: several distinct
    // plain SemVer versions of the same package/role must all survive
    // as separate BTreeSet entries too, not just the grammar-collision
    // edge case above.
    let mut ordinary_ids = BTreeSet::new();
    for raw in ["1.0.0", "1.2.3", "2.0.0", "0.9.9"] {
        let id = ReleaseOperationId::tag(
            ReleasePackageId::parse("cargo/demo").unwrap(),
            Version::parse(raw, VersionGrammar::SemVer).unwrap(),
        );
        assert!(
            ordinary_ids.insert(id),
            "version {raw} must not collide with a prior entry"
        );
    }
    assert_eq!(ordinary_ids.len(), 4);
}

/// Sibling instance of the same `.render()`-only defect, found in
/// `ArtifactSlotId::cmp` (a different type in the same file) while
/// auditing `ReleaseOperationId` above. Same construction: two slots
/// identical in every field except `version.grammar()`.
#[test]
fn artifact_slot_ids_differing_only_by_version_grammar_stay_distinct_under_ord() {
    let package = ReleasePackageId::parse("cargo/demo").unwrap();
    let v_semver = Version::parse("1.2.3", VersionGrammar::SemVer).unwrap();
    let v_pep440 = Version::parse("1.2.3", VersionGrammar::Pep440).unwrap();
    assert_eq!(v_semver.render(), v_pep440.render());

    let slot_a = ArtifactSlotId::new(
        package.clone(),
        v_semver,
        "x86_64-unknown-linux-gnu",
        "demo.tar.gz",
        GitHubRepository::parse("orin-dx/callisto").unwrap(),
        ".github/workflows/release.yml",
        CommitSha::parse(&"a".repeat(40)).unwrap(),
    )
    .unwrap();
    let slot_b = ArtifactSlotId::new(
        package,
        v_pep440,
        "x86_64-unknown-linux-gnu",
        "demo.tar.gz",
        GitHubRepository::parse("orin-dx/callisto").unwrap(),
        ".github/workflows/release.yml",
        CommitSha::parse(&"a".repeat(40)).unwrap(),
    )
    .unwrap();

    assert_ne!(slot_a, slot_b, "distinct version grammars must stay Eq-distinct");
    assert_ne!(
        slot_a.cmp(&slot_b),
        Ordering::Equal,
        "distinct version grammars must not compare Ord::Equal"
    );

    let mut slots = BTreeSet::new();
    assert!(slots.insert(slot_a), "first slot must insert");
    assert!(
        slots.insert(slot_b),
        "second slot must NOT be displaced by the first in a BTreeSet"
    );
    assert_eq!(slots.len(), 2);
}

#[test]
fn transcript_has_a_fixed_sha256_vector() {
    let mut transcript = CanonicalTranscript::semantic_input_v1();
    transcript.push_str("package", "cargo/callisto-model");
    transcript.push_str("version", "0.5.0");

    assert_eq!(
        SemanticInputDigest::from_transcript(&transcript).to_string(),
        "9dfd667345a133d98c1ae1ed8984b4161d8f27f310c09412b136007331201ec4"
    );
}

#[test]
fn transcript_length_and_field_boundaries_affect_the_digest() {
    let mut split = CanonicalTranscript::semantic_input_v1();
    split.push_str("role", "publish");
    split.push_str("edge", "a->b");

    let mut joined = CanonicalTranscript::semantic_input_v1();
    joined.push_str("role", "publisha->b");

    assert_ne!(
        SemanticInputDigest::from_transcript(&split),
        SemanticInputDigest::from_transcript(&joined)
    );
}

#[test]
fn transcript_domain_separation_prevents_decision_intent_and_snapshot_collisions() {
    let mut snapshot = CanonicalTranscript::semantic_input_v1();
    snapshot.push_str("role", "publish");
    let mut decision = CanonicalTranscript::decision_v1();
    decision.push_str("role", "publish");
    let mut intent = CanonicalTranscript::intent_v1();
    intent.push_str("role", "publish");

    assert_ne!(
        SemanticInputDigest::from_transcript(&snapshot).as_str(),
        IntentDigest::from_transcript(&intent).as_str()
    );
    assert_ne!(
        DecisionDigest::from_transcript(&decision).as_str(),
        IntentDigest::from_transcript(&intent).as_str()
    );
}

#[test]
fn platform_publish_round_trips_under_its_owner_and_rejects_unsafe_platforms() {
    let owner = ReleasePackageId::parse("npm/@s/cli").unwrap();
    let platform = PlatformPackageV1::new(
        ReleasePackageId::parse("npm/@s/cli-linux-x64-gnu").unwrap(),
        "packages/cli/npm/linux-x64-gnu",
    )
    .unwrap();
    let id = ReleaseOperationId::platform_publish(
        owner.clone(),
        Version::semver(1, 2, 3),
        registry_binding("npm"),
        platform,
    );
    assert_eq!(id.package, owner);
    let wire = serde_json::to_value(&id).unwrap();
    assert_eq!(wire["role"]["kind"], "platformPublish");
    assert_eq!(wire["role"]["platform"]["directory"], "packages/cli/npm/linux-x64-gnu");
    assert_eq!(serde_json::from_value::<ReleaseOperationId>(wire.clone()).unwrap(), id);

    for directory in ["", "/abs", "-flag", "a/../b", "a//b", "./a", "a b"] {
        let mut bad = wire.clone();
        bad["role"]["platform"]["directory"] = directory.into();
        assert!(
            serde_json::from_value::<ReleaseOperationId>(bad).is_err(),
            "{directory:?}"
        );
    }
    assert!(PlatformPackageV1::new(ReleasePackageId::parse("cargo/x").unwrap(), "x").is_err());
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
fn release_operation_orders_exactly_and_rejects_invalid_prerequisites() {
    let package = ReleasePackageId::parse("cargo/callisto-model").unwrap();
    let version = Version::semver(1, 2, 3);
    let publish =
        ReleaseOperation::registry_publish(package.clone(), version.clone(), registry_binding("cratesIo"), vec![])
            .unwrap();
    let tag = ReleaseOperation::tag(package, version, vec![publish.id().clone()]).unwrap();

    assert!(publish.id() < tag.id());
    assert!(ReleaseOperation::tag(
        ReleasePackageId::parse("cargo/callisto-model").unwrap(),
        Version::semver(1, 2, 3),
        vec![tag.id().clone()],
    )
    .is_err());
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
fn release_run_envelope_wire_fails_closed() {
    let intent = test_intent(
        ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]),
        ExecutionTrustProfileV1::GitCommit,
        vec![ReleaseOperation::tag(
            ReleasePackageId::parse("cargo/callisto-model").unwrap(),
            Version::semver(1, 2, 3),
            vec![],
        )
        .unwrap()],
    )
    .unwrap();
    let envelope = test_envelope(&intent);
    assert_ne!(
        envelope.orchestration_revision(),
        envelope.release_source_revision(),
        "a run must retain distinct coordinator and release-source identities"
    );
    let mut wire = serde_json::to_value(&envelope).unwrap();
    wire["schemaVersion"] = serde_json::Value::from(9);
    assert!(serde_json::from_value::<ReleaseRunEnvelopeV1>(wire).is_err());
}

#[test]
fn receipt_is_bound_to_intent_and_requires_terminal_outcomes() {
    let package = ReleasePackageId::parse("cargo/callisto-model").unwrap();
    let operation =
        ReleaseOperation::registry_publish(package, Version::semver(1, 2, 3), registry_binding("cratesIo"), vec![])
            .unwrap();
    let intent = test_intent(
        ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("c".repeat(40)).unwrap(), vec![]),
        ExecutionTrustProfileV1::GitCommit,
        vec![operation.clone()],
    )
    .unwrap();
    let pending = pending_state(&intent);
    assert!(ReleaseReceiptV1::from_state(&intent, &pending).is_err());

    let mut complete = pending;
    publish_operation(&mut complete, operation.id());
    let receipt = ReleaseReceiptV1::from_state(&intent, &complete).unwrap();
    assert_eq!(receipt.intent_digest(), intent.digest());

    let mut failed = pending_state(&intent);
    failed.apply(operation.id(), &attempt_event()).unwrap();
    failed
        .apply(
            operation.id(),
            &OperationEvent::EffectFailedAndAbsent {
                proof: ProviderObservationV1::Absent.absent_proof().unwrap(),
            },
        )
        .unwrap();
    assert!(matches!(
        ReleaseReceiptV1::from_state(&intent, &failed),
        Err(ReleaseReceiptError::NonSuccessfulOperation { .. })
    ));
    assert_eq!(
        receipt.observation(operation.id()),
        Some(&exact_observation(operation.id())),
        "the receipt must record the evidence persisted at confirmation"
    );
}

#[test]
fn apply_rejects_success_evidence_of_another_role() {
    let operation = ReleaseOperation::registry_publish(
        ReleasePackageId::parse("cargo/callisto-model").unwrap(),
        Version::semver(1, 2, 3),
        registry_binding("cratesIo"),
        vec![],
    )
    .unwrap();
    let intent = test_intent(
        ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("c".repeat(40)).unwrap(), vec![]),
        ExecutionTrustProfileV1::GitCommit,
        vec![operation.clone()],
    )
    .unwrap();
    let mut state = pending_state(&intent);
    let tag_evidence = ProviderObservationV1::Exact {
        evidence: ProviderEvidenceV1::GitTag {
            peeled_commit: CommitSha::parse(&"a".repeat(40)).unwrap(),
        },
    };
    let event = OperationEvent::ObservedExactBeforeEffect {
        evidence: tag_evidence.exact_evidence().unwrap(),
    };
    assert!(matches!(
        state.apply(operation.id(), &event),
        Err(ReleaseStateError::EvidenceRejected(_))
    ));
    assert_eq!(state.operation_state(operation.id()), Some(OperationState::Pending));
    assert_eq!(state.operation_evidence(operation.id()), None);
}

#[test]
fn receipt_wire_rejects_unknown_schema() {
    let operation = ReleaseOperation::registry_publish(
        ReleasePackageId::parse("cargo/callisto-model").unwrap(),
        Version::semver(1, 2, 3),
        registry_binding("cratesIo"),
        vec![],
    )
    .unwrap();
    let intent = test_intent(
        ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("e".repeat(40)).unwrap(), vec![]),
        ExecutionTrustProfileV1::GitCommit,
        vec![operation.clone()],
    )
    .unwrap();
    let mut complete = pending_state(&intent);
    publish_operation(&mut complete, operation.id());
    let receipt = ReleaseReceiptV1::from_state(&intent, &complete).unwrap();
    let mut receipt_wire = serde_json::to_value(receipt).unwrap();
    receipt_wire["schemaVersion"] = serde_json::Value::from(9);
    assert!(serde_json::from_value::<ReleaseReceiptV1>(receipt_wire).is_err());
}

#[test]
fn state_and_receipt_require_exact_intent_digest_and_roster() {
    let package = ReleasePackageId::parse("cargo/callisto-model").unwrap();
    let operation = ReleaseOperation::tag(package.clone(), Version::semver(1, 2, 3), vec![]).unwrap();
    let different_operation = ReleaseOperation::forge_release(package, Version::semver(1, 2, 3), vec![]).unwrap();
    let first = test_intent(
        ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("f".repeat(40)).unwrap(), vec![]),
        ExecutionTrustProfileV1::GitCommit,
        vec![operation.clone()],
    )
    .unwrap();
    let different_digest = test_intent(
        ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("0".repeat(40)).unwrap(), vec![]),
        ExecutionTrustProfileV1::GitCommit,
        vec![operation.clone()],
    )
    .unwrap();
    let state = pending_state(&first);
    assert!(matches!(
        state.validate_for_intent(&different_digest),
        Err(ReleaseStateError::MismatchedIntent)
    ));

    let mut complete = state;
    publish_operation(&mut complete, operation.id());
    let receipt = ReleaseReceiptV1::from_state(&first, &complete).unwrap();
    assert!(matches!(
        receipt.validate_for_intent(&different_digest),
        Err(ReleaseReceiptError::MismatchedIntent)
    ));
    let mut wrong_receipt_wire = serde_json::to_value(&receipt).unwrap();
    wrong_receipt_wire["outcomes"][0]["operation"] = serde_json::to_value(different_operation.id()).unwrap();
    let wrong_receipt = serde_json::from_value::<ReleaseReceiptV1>(wrong_receipt_wire).unwrap();
    assert!(matches!(
        wrong_receipt.validate_for_intent(&first),
        Err(ReleaseReceiptError::MismatchedOperationRoster)
    ));
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

/// A release package name becomes an argv word (`npm view <name>@<v>`,
/// `pnpm publish --filter=<name>`) and a URL path segment (the cargo
/// sparse index, the PyPI JSON API). These are the shapes that would
/// change the meaning of one of those, so they are refused at plan time.
#[test]
fn release_package_names_reject_argv_and_url_hazards() {
    let hazards = [
        (Ecosystem::Cargo, "-x"),
        (Ecosystem::Cargo, "--registry"),
        (Ecosystem::Cargo, "a b"),
        (Ecosystem::Cargo, "a\nb"),
        (Ecosystem::Cargo, "a\u{0}b"),
        (Ecosystem::Cargo, "a/b"),
        (Ecosystem::Cargo, "a%2fb"),
        (Ecosystem::Cargo, "a?b"),
        (Ecosystem::Cargo, "a#b"),
        (Ecosystem::Cargo, "a\\b"),
        (Ecosystem::Cargo, "1crate"),
        (Ecosystem::Cargo, "crate.name"),
        (Ecosystem::Npm, "-x"),
        (Ecosystem::Npm, "@scope"),
        (Ecosystem::Npm, "@scope/a/b"),
        (Ecosystem::Npm, "scope/name"),
        (Ecosystem::Npm, ".hidden"),
        (Ecosystem::Npm, "_hidden"),
        (Ecosystem::Npm, "@scope/-x"),
        (Ecosystem::Pypi, "-x"),
        (Ecosystem::Pypi, "pkg/../etc"),
        (Ecosystem::Pypi, ".pkg"),
        (Ecosystem::Pypi, "pkg-"),
        (Ecosystem::Pypi, "pkg name"),
    ];
    for (ecosystem, name) in hazards {
        assert!(
            matches!(
                ReleasePackageId::new(ecosystem, name),
                Err(ReleasePackageIdParseError::UnsafeName { .. } | ReleasePackageIdParseError::Malformed { .. })
            ),
            "{ecosystem:?} accepted an unsafe package name: {name:?}"
        );
    }
}

/// The real names each ecosystem publishes under must keep working.
#[test]
fn release_package_names_accept_each_ecosystem_grammar() {
    for (ecosystem, name) in [
        (Ecosystem::Cargo, "callisto-model"),
        (Ecosystem::Cargo, "_private_crate"),
        (Ecosystem::Cargo, "serde"),
        (Ecosystem::Npm, "callisto"),
        (Ecosystem::Npm, "@orin-dx/callisto"),
        (Ecosystem::Npm, "left.pad"),
        (Ecosystem::Npm, "JSONStream"),
        (Ecosystem::Npm, "Base64"),
        (Ecosystem::Npm, "@_scope/_x"),
        (Ecosystem::Npm, "@types/node"),
        (Ecosystem::Pypi, "typing-extensions"),
        (Ecosystem::Pypi, "zope.interface"),
        (Ecosystem::Pypi, "Flask"),
    ] {
        let id = ReleasePackageId::new(ecosystem, name)
            .unwrap_or_else(|error| panic!("{ecosystem:?}/{name} must stay valid: {error}"));
        assert_eq!(id.name(), name);
    }
}

/// `.` names the directory an asset path is joined onto, not a file in it.
#[test]
fn artifact_slot_components_reject_dot_and_dot_dot() {
    let slot = |platform: &str, asset: &str| {
        ArtifactSlotId::new(
            ReleasePackageId::new(Ecosystem::Cargo, "demo").unwrap(),
            Version::semver(1, 0, 0),
            platform,
            asset,
            GitHubRepository::parse("orin-dx/callisto").unwrap(),
            ".github/workflows/release.yml",
            CommitSha::parse(&"b".repeat(40)).unwrap(),
        )
    };
    for (platform, asset) in [(".", "a.tar.gz"), ("linux", "."), ("..", "a.tar.gz"), ("linux", "..")] {
        assert!(
            matches!(slot(platform, asset), Err(ArtifactSlotError::UnsafeSlotComponent)),
            "unsafe slot component accepted: {platform:?}/{asset:?}"
        );
    }
    assert!(slot("x86_64-unknown-linux-gnu", "callisto.tar.gz").is_ok());
}

/// `ReleasePlanReport` must flatten `ReleaseIntentV1`'s own fields (not nest them
/// under an `intent` key) and add a `diagnostics` array only when non-empty --
/// the one JSON envelope `output::emit_report` gives every command alike.
#[test]
fn release_plan_report_flattens_intent_and_omits_empty_diagnostics() {
    let package = ReleasePackageId::new(Ecosystem::Cargo, "demo").unwrap();
    let version = Version::semver(1, 0, 0);
    let tag = ReleaseOperation::tag(package, version, vec![]).unwrap();
    let intent = test_intent(
        Ok(ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]).unwrap()),
        ExecutionTrustProfileV1::GitCommit,
        vec![tag],
    )
    .unwrap();
    let report = ReleasePlanReport {
        intent: intent.clone(),
        diagnostics: Vec::new(),
    };

    let value = serde_json::to_value(&report).unwrap();
    assert_eq!(value["digest"], serde_json::to_value(intent.digest()).unwrap());
    assert!(value.get("intent").is_none(), "must flatten, not nest: {value}");
    assert!(
        value.get("diagnostics").is_none(),
        "empty diagnostics must be omitted: {value}"
    );
    assert_eq!(
        crate::report::Report::schema_version(&report),
        u32::from(ReleaseIntentV1::SCHEMA_VERSION)
    );
}
