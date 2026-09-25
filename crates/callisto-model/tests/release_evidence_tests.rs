use callisto_model::*;

fn sha(c: char) -> CommitSha {
    CommitSha::parse(&c.to_string().repeat(40)).unwrap()
}

fn intent() -> ReleaseIntentV1 {
    intent_at(Version::semver(1, 2, 3))
}

fn intent_at(version: Version) -> ReleaseIntentV1 {
    let package = ReleasePackageId::parse("cargo/demo").unwrap();
    let op = ReleaseOperation::tag(package.clone(), version.clone(), vec![]).unwrap();
    ReleaseIntentV1::new(
        ReleaseDecisionV1::new(vec![ReleaseDecisionEntry {
            package,
            target_version: version,
            reasons: vec![ReleaseInclusionReason::ExplicitSelection],
        }])
        .unwrap(),
        ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]).unwrap(),
        ExecutionTrustProfileV1::GitCommit,
        vec![op],
        vec![],
    )
    .unwrap()
}

fn envelope(i: &ReleaseIntentV1) -> ReleaseRunEnvelopeV1 {
    ReleaseRunEnvelopeV1::new(sha('b'), i, None).unwrap()
}
fn tag_evidence() -> ProviderEvidenceV1 {
    ProviderEvidenceV1::GitTag {
        peeled_commit: sha('a'),
    }
}
fn done_state(i: &ReleaseIntentV1) -> ReleaseExecutionStateV1 {
    let mut st = ReleaseExecutionStateV1::new(i, envelope(i)).unwrap();
    for o in &i.operations {
        st.apply(
            o.id(),
            &OperationEvent::Attempt {
                proof: ProviderObservationV1::Absent.absent_proof().unwrap(),
            },
        )
        .unwrap();
        st.apply(
            o.id(),
            &OperationEvent::Confirmed {
                evidence: ProviderObservationV1::Exact {
                    evidence: tag_evidence(),
                }
                .exact_evidence()
                .unwrap(),
            },
        )
        .unwrap();
    }
    st
}
#[test]
fn intent_has_no_profile_and_rejects_one_on_the_wire() {
    let mut v = serde_json::to_value(intent()).unwrap();
    assert!(v.get("profile").is_none());
    assert_eq!(v["schemaVersion"], 5);
    v["profile"] = "production".into();
    assert!(serde_json::from_value::<ReleaseIntentV1>(v).is_err());
}

#[test]
fn intent_of_another_schema_version_names_the_found_version() {
    for found in [4u8, 6] {
        let mut v = serde_json::to_value(intent()).unwrap();
        v["schemaVersion"] = found.into();
        let error = serde_json::from_value::<ReleaseIntentV1>(v).unwrap_err().to_string();
        assert!(error.contains(&format!("schema version {found}")), "{error}");
    }
}

#[test]
fn envelope_has_no_profile_and_other_schema_versions_name_the_found_version() {
    let i = intent();
    let v = serde_json::to_value(envelope(&i)).unwrap();
    assert!(v.get("profile").is_none());
    assert_eq!(v["schemaVersion"], 3);
    let mut with_profile = v.clone();
    with_profile["profile"] = "production".into();
    assert!(serde_json::from_value::<ReleaseRunEnvelopeV1>(with_profile).is_err());
    for found in [2u8, 4] {
        let mut wire = v.clone();
        wire["schemaVersion"] = found.into();
        let error = serde_json::from_value::<ReleaseRunEnvelopeV1>(wire)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&format!("schema version {found}")), "{error}");
    }
}

#[test]
fn receipt_rejects_a_state_bound_to_another_intent() {
    let i = intent();
    let st = done_state(&i);
    assert!(ReleaseReceiptV1::from_state(&i, &st).is_ok());

    let other = intent_at(Version::semver(1, 2, 4));
    assert!(matches!(
        ReleaseReceiptV1::from_state(&other, &st),
        Err(ReleaseReceiptError::InvalidState(ReleaseStateError::MismatchedIntent))
    ));
}

#[test]
fn receipt_records_the_evidence_in_state_and_needs_every_operation_terminal() {
    let i = intent();
    let st = done_state(&i);
    let receipt = ReleaseReceiptV1::from_state(&i, &st).unwrap();
    for o in &i.operations {
        assert_eq!(
            receipt.observation(o.id()),
            Some(&ProviderObservationV1::Exact {
                evidence: tag_evidence()
            })
        );
    }
    let pending = ReleaseExecutionStateV1::new(&i, envelope(&i)).unwrap();
    assert!(matches!(
        ReleaseReceiptV1::from_state(&i, &pending),
        Err(ReleaseReceiptError::NonTerminalOperation { .. })
    ));
}

#[test]
fn deserialized_receipt_rejects_nonexact_or_missing_observations() {
    let i = intent();
    let r = ReleaseReceiptV1::from_state(&i, &done_state(&i)).unwrap();
    let good = serde_json::to_value(&r).unwrap();
    let roundtrip: ReleaseReceiptV1 = serde_json::from_value(good.clone()).unwrap();
    assert!(roundtrip.validate_for_intent(&i).is_ok());
    let mut nonexact = good.clone();
    nonexact["observations"][0]["observation"] =
        serde_json::json!({ "kind": "indeterminate", "cause": { "kind": "commandFailed" } });
    assert!(serde_json::from_value::<ReleaseReceiptV1>(nonexact).is_err());
    let mut missing = good;
    missing["observations"] = serde_json::json!([]);
    let parsed: ReleaseReceiptV1 = serde_json::from_value(missing).unwrap();
    assert!(matches!(
        parsed.validate_for_intent(&i),
        Err(ReleaseReceiptError::MismatchedObservationRoster)
    ));
}

#[test]
fn artifact_manifest_rejects_attestation_source_that_is_not_the_coordinator_revision() {
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
    let i = ReleaseIntentV1::new(
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
    let digest = ArtifactDigest::from_bytes(b"bytes");
    let entry = |source: CommitSha| ArtifactManifestEntryV1 {
        slot: slot.clone(),
        digest: digest.clone(),
        byte_length: 5,
        attestation: GitHubArtifactAttestationV1 {
            repository: slot.attestation_policy.repository.clone(),
            workflow_path: slot.attestation_policy.workflow_path.clone(),
            workflow_commit: slot.attestation_policy.workflow_commit.clone(),
            subject_digest: digest.clone(),
            source_commit: source,
        },
    };
    assert!(ArtifactManifestV1::new(&i, vec![entry(sha('b'))]).is_ok());
    assert!(
        ArtifactManifestV1::new(&i, vec![entry(sha('a'))]).is_err(),
        "attestation source must be the coordinator workflow revision"
    );
}

#[test]
fn red_c6_manifest_with_forged_source_commit_is_rejected() {
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
    let i = ReleaseIntentV1::new(
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
    let digest = ArtifactDigest::from_bytes(b"bytes");
    let entry = ArtifactManifestEntryV1 {
        slot: slot.clone(),
        digest: digest.clone(),
        byte_length: 5,
        attestation: GitHubArtifactAttestationV1 {
            repository: slot.attestation_policy.repository.clone(),
            workflow_path: slot.attestation_policy.workflow_path.clone(),
            workflow_commit: slot.attestation_policy.workflow_commit.clone(),
            subject_digest: digest.clone(),
            source_commit: sha('b'),
        },
    };
    let manifest = ArtifactManifestV1::new(&i, vec![entry]).unwrap();
    assert!(manifest.validate_for_intent(&i).is_ok());
    let mut forged = manifest.clone();
    forged.source_commit = sha('f');
    assert!(
        forged.validate_for_intent(&i).is_err(),
        "forged source_commit must be rejected"
    );
}
