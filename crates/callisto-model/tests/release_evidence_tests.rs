use callisto_model::*;

fn sha(c: char) -> CommitSha {
    CommitSha::parse(&c.to_string().repeat(40)).unwrap()
}

fn intent(profile: &str) -> ReleaseIntentV1 {
    let package = ReleasePackageId::parse("cargo/demo").unwrap();
    let version = Version::semver(1, 2, 3);
    let op = ReleaseOperation::tag(package.clone(), version.clone(), vec![]).unwrap();
    ReleaseIntentV1::new(
        ReleaseProfileId::parse(profile).unwrap(),
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
    ReleaseRunEnvelopeV1::new(ReleaseRunKindV1::Initial, sha('b'), i, None).unwrap()
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
fn exact(i: &ReleaseIntentV1) -> Vec<ReleaseOperationObservationV1> {
    i.operations
        .iter()
        .map(|o| {
            ReleaseOperationObservationV1::new(
                o.id().clone(),
                ProviderObservationV1::Exact {
                    evidence: tag_evidence(),
                },
            )
            .unwrap()
        })
        .collect()
}

#[test]
fn profile_is_bound_into_the_intent_digest_and_cannot_be_relabelled() {
    let prod = intent("production");
    let reh = intent("rehearsal");
    assert_ne!(prod.digest(), reh.digest());
    let mut v = serde_json::to_value(&prod).unwrap();
    v["profile"] = "rehearsal".into();
    assert!(
        serde_json::from_value::<ReleaseIntentV1>(v).is_err(),
        "relabelled profile must fail digest verification"
    );
}

#[test]
fn receipt_rejects_a_state_whose_envelope_belongs_to_another_profile_or_intent() {
    let i = intent("production");
    let st = done_state(&i);
    assert!(ReleaseReceiptV1::from_evidence(&i, &st, exact(&i)).is_ok());

    // The envelope is derived from its intent, so a mismatch can only be
    // forged on the wire -- and is then rejected on the way back in.
    let mut forged = serde_json::to_value(&st).unwrap();
    forged["envelope"]["profile"] = "rehearsal".into();
    let forged: ReleaseExecutionStateV1 = serde_json::from_value(forged).unwrap();
    assert!(matches!(
        ReleaseReceiptV1::from_evidence(&i, &forged, exact(&i)),
        Err(ReleaseReceiptError::InvalidState(ReleaseStateError::InvalidEnvelope(
            ReleaseRunEnvelopeError::MismatchedProfile
        )))
    ));

    let other = intent("rehearsal");
    assert!(matches!(
        ReleaseReceiptV1::from_evidence(&other, &st, exact(&i)),
        Err(ReleaseReceiptError::InvalidState(ReleaseStateError::MismatchedIntent))
    ));
}

#[test]
fn receipt_requires_a_complete_duplicate_free_exact_observation_roster() {
    let i = intent("production");
    let st = done_state(&i);
    assert!(matches!(
        ReleaseReceiptV1::from_evidence(&i, &st, vec![]),
        Err(ReleaseReceiptError::MismatchedObservationRoster)
    ));
    let mut dup = exact(&i);
    dup.extend(exact(&i));
    assert!(matches!(
        ReleaseReceiptV1::from_evidence(&i, &st, dup),
        Err(ReleaseReceiptError::DuplicateObservation { .. })
    ));
    for bad in [
        ProviderObservationV1::Absent,
        ProviderObservationV1::Conflict {
            reason: ProviderConflictReason::RemoteTagTargetDiffers,
        },
        ProviderObservationV1::Indeterminate {
            cause: ProviderIndeterminateCause::CommandFailed,
        },
    ] {
        let obs = vec![ReleaseOperationObservationV1::new(i.operations[0].id().clone(), bad).unwrap()];
        assert!(matches!(
            ReleaseReceiptV1::from_evidence(&i, &st, obs),
            Err(ReleaseReceiptError::NonExactObservation { .. })
        ));
    }
}

#[test]
fn deserialized_receipt_rejects_nonexact_or_missing_observations() {
    let i = intent("production");
    let r = ReleaseReceiptV1::from_evidence(&i, &done_state(&i), exact(&i)).unwrap();
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
        ".github/workflows/callisto-release.yml",
        sha('b'),
    )
    .unwrap();
    let i = ReleaseIntentV1::new(
        ReleaseProfileId::production(),
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
        ".github/workflows/callisto-release.yml",
        sha('b'),
    )
    .unwrap();
    let i = ReleaseIntentV1::new(
        ReleaseProfileId::production(),
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
