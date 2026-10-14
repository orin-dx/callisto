use std::cmp::Ordering;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use super::{
    version_ord_key, ArtifactDigest, GitHubAttestationPolicyV1, GitHubRepository, IntentDigest, ReleaseIntentV1,
    ReleasePackageId, SourceIdentity,
};
use crate::{CommitSha, Version};

/// Exact binary asset declaration authorized by a durable release intent.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactSlotId {
    pub package: ReleasePackageId,
    pub version: Version,
    pub platform: String,
    pub asset_name: String,
    pub attestation_policy: GitHubAttestationPolicyV1,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArtifactSlotIdWire {
    package: ReleasePackageId,
    version: String,
    platform: String,
    asset_name: String,
    attestation_policy: GitHubAttestationPolicyV1,
}

impl<'de> Deserialize<'de> for ArtifactSlotId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ArtifactSlotIdWire::deserialize(deserializer)?;
        let version =
            Version::parse_for_ecosystem(&wire.version, wire.package.ecosystem()).map_err(serde::de::Error::custom)?;
        Self::new(
            wire.package,
            version,
            wire.platform,
            wire.asset_name,
            wire.attestation_policy.repository,
            wire.attestation_policy.workflow_path,
            wire.attestation_policy.workflow_commit,
        )
        .map_err(serde::de::Error::custom)
    }
}

impl ArtifactSlotId {
    pub fn new(
        package: ReleasePackageId,
        version: Version,
        platform: impl Into<String>,
        asset_name: impl Into<String>,
        repository: GitHubRepository,
        workflow_path: impl Into<String>,
        workflow_commit: CommitSha,
    ) -> Result<Self, ArtifactSlotError> {
        let platform = platform.into();
        let asset_name = asset_name.into();
        let policy = GitHubAttestationPolicyV1::new(repository, workflow_path, workflow_commit)?;
        if !is_safe_artifact_component(&platform) || !is_safe_artifact_component(&asset_name) {
            return Err(ArtifactSlotError::UnsafeSlotComponent);
        }
        Ok(Self {
            package,
            version,
            platform,
            asset_name,
            attestation_policy: policy,
        })
    }
}

impl PartialOrd for ArtifactSlotId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// `version` goes through `version_ord_key` (grammar + raw), not bare `render()` --
// same residual-`.render()` defect as `ReleaseOperationId::cmp` above; see
// `version_ord_key`'s doc comment for why bare `render()` breaks the
// `a.cmp(b) == Equal` iff `a == b` invariant `BTreeSet`/`BTreeMap` require.
impl Ord for ArtifactSlotId {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            &self.package,
            version_ord_key(&self.version),
            &self.platform,
            &self.asset_name,
            &self.attestation_policy.repository,
            &self.attestation_policy.workflow_path,
            &self.attestation_policy.workflow_commit,
        )
            .cmp(&(
                &other.package,
                version_ord_key(&other.version),
                &other.platform,
                &other.asset_name,
                &other.attestation_policy.repository,
                &other.attestation_policy.workflow_path,
                &other.attestation_policy.workflow_commit,
            ))
    }
}

pub fn is_safe_artifact_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.contains("..")
        // `.` names the directory being joined, not a file in it.
        && value != "."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Credential-free GitHub attestation policy and verified provenance facts.
///
/// `source_commit` is the source digest GitHub records for the workflow run:
/// the coordinator revision that executed the attestation action. The release
/// source is separately bound by `ArtifactManifestV1::source_commit` and the
/// immutable intent, because recovery intentionally builds an older release
/// checkout with current coordinator workflow code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitHubArtifactAttestationV1 {
    pub repository: GitHubRepository,
    pub workflow_path: String,
    pub workflow_commit: CommitSha,
    pub subject_digest: ArtifactDigest,
    pub source_commit: CommitSha,
}

/// One exact built asset and its verified provenance binding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactManifestEntryV1 {
    pub slot: ArtifactSlotId,
    pub digest: ArtifactDigest,
    pub byte_length: u64,
    pub attestation: GitHubArtifactAttestationV1,
}

/// Typed build output bound to one durable intent and source commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactManifestV1 {
    pub schema_version: u8,
    pub intent_digest: IntentDigest,
    pub source_commit: CommitSha,
    pub entries: Vec<ArtifactManifestEntryV1>,
}

impl ArtifactManifestV1 {
    pub const SCHEMA_VERSION: u8 = 1;

    pub fn new(
        intent: &ReleaseIntentV1,
        mut entries: Vec<ArtifactManifestEntryV1>,
    ) -> Result<Self, ArtifactManifestError> {
        let SourceIdentity::GitCommit { sha } = &intent.snapshot.source else {
            return Err(ArtifactManifestError::NonGitSource);
        };
        entries.sort_by(|left, right| left.slot.cmp(&right.slot));
        if entries.windows(2).any(|pair| pair[0].slot == pair[1].slot) {
            return Err(ArtifactManifestError::DuplicateSlot);
        }
        let expected = &intent.artifact_slots;
        if entries.iter().map(|entry| &entry.slot).ne(expected.iter()) {
            return Err(ArtifactManifestError::MismatchedSlotRoster);
        }
        if entries.iter().any(|entry| {
            entry.digest != entry.attestation.subject_digest
                || entry.attestation.source_commit != entry.slot.attestation_policy.workflow_commit
                || entry.attestation.repository != entry.slot.attestation_policy.repository
                || entry.attestation.workflow_path != entry.slot.attestation_policy.workflow_path
                || entry.attestation.workflow_commit != entry.slot.attestation_policy.workflow_commit
        }) {
            return Err(ArtifactManifestError::MismatchedAttestation);
        }
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            intent_digest: intent.digest.clone(),
            source_commit: sha.clone(),
            entries,
        })
    }

    /// Hashes the canonical serialized manifest transported from build to
    /// execution. Its entry roster was canonicalized by [`Self::new`].
    pub fn digest(&self) -> ArtifactDigest {
        ArtifactDigest::from_bytes(serde_json::to_vec(self).expect("artifact manifest serializes"))
    }

    pub fn validate_for_intent(&self, intent: &ReleaseIntentV1) -> Result<(), ArtifactManifestError> {
        if self.schema_version != Self::SCHEMA_VERSION || self.intent_digest != intent.digest {
            return Err(ArtifactManifestError::MismatchedIntent);
        }
        match &intent.snapshot.source {
            SourceIdentity::GitCommit { sha } if sha == &self.source_commit => {}
            SourceIdentity::GitCommit { .. } => return Err(ArtifactManifestError::MismatchedSourceCommit),
            SourceIdentity::HermeticContent { .. } => return Err(ArtifactManifestError::NonGitSource),
        }
        Self::new(intent, self.entries.clone()).map(|_| ())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ArtifactSlotError {
    #[error("artifact slot platform and asset name must be safe relative identifiers")]
    UnsafeSlotComponent,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ArtifactManifestError {
    #[error("artifact manifests require a Git commit source")]
    NonGitSource,
    #[error("artifact manifest repeats an artifact slot")]
    DuplicateSlot,
    #[error("artifact manifest slot roster differs from the intent")]
    MismatchedSlotRoster,
    #[error("artifact attestation does not bind its exact bytes and intent source commit")]
    MismatchedAttestation,
    #[error("artifact manifest is bound to a different intent or schema")]
    MismatchedIntent,
    #[error("artifact manifest source commit differs from the intent release source")]
    MismatchedSourceCommit,
}

#[cfg(test)]
mod tests {
    use crate::{CommitSha, Ecosystem};
    use std::cmp::Ordering;
    use std::collections::BTreeSet;

    use super::super::test_support::{native_intent_with_slot, native_versions, test_intent_with_slots};
    use super::*;
    use crate::release::*;

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
}
