use std::cmp::Ordering;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use super::{
    version_ord_key, ArtifactDigest, GitHubAttestationPolicyV1, GitHubRepository, IntentDigest, ReleaseIntentV1,
    ReleaseOperationRole, ReleasePackageId, SourceIdentity,
};
use crate::{is_pypi_sdist, is_pypi_wheel, pypi_dist_belongs_to, CommitSha, Ecosystem, Version, VersionGrammar};

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

/// One built Python distribution file, sealed by its digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DistributionFileV1 {
    pub filename: String,
    pub digest: ArtifactDigest,
    pub byte_length: u64,
}

/// The sdist and wheels one build produced for one Python release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DistributionSetV1 {
    pub package: ReleasePackageId,
    pub version: String,
    pub files: Vec<DistributionFileV1>,
}

impl DistributionSetV1 {
    fn key(&self) -> (&ReleasePackageId, &str) {
        (&self.package, &self.version)
    }
}

/// Typed build output bound to one durable intent and source commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactManifestV1 {
    pub schema_version: u8,
    pub intent_digest: IntentDigest,
    pub source_commit: CommitSha,
    pub entries: Vec<ArtifactManifestEntryV1>,
    /// Sealed Python file sets: empty, or one per Python registry publish of the intent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub distributions: Vec<DistributionSetV1>,
}

impl ArtifactManifestV1 {
    pub const SCHEMA_VERSION: u8 = 1;

    pub fn new(intent: &ReleaseIntentV1, entries: Vec<ArtifactManifestEntryV1>) -> Result<Self, ArtifactManifestError> {
        Self::with_distributions(intent, entries, Vec::new())
    }

    pub fn with_distributions(
        intent: &ReleaseIntentV1,
        mut entries: Vec<ArtifactManifestEntryV1>,
        mut distributions: Vec<DistributionSetV1>,
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
        distributions.sort_by(|left, right| left.key().cmp(&right.key()));
        for set in &mut distributions {
            set.files.sort_by(|left, right| left.filename.cmp(&right.filename));
        }
        validate_distributions(intent, &entries, &distributions)?;
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            intent_digest: intent.digest.clone(),
            source_commit: sha.clone(),
            entries,
            distributions,
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
        Self::with_distributions(intent, self.entries.clone(), self.distributions.clone()).map(|_| ())
    }
}

/// The versions the intent publishes to a Python registry.
pub fn python_publishes(intent: &ReleaseIntentV1) -> Vec<(&ReleasePackageId, String)> {
    let mut publishes: Vec<_> = intent
        .operations
        .iter()
        .map(|operation| operation.id())
        .filter(|id| id.package.ecosystem() == Ecosystem::Pypi)
        .filter(|id| matches!(id.role, ReleaseOperationRole::RegistryPublish { .. }))
        .map(|id| (&id.package, id.version.render().to_string()))
        .collect();
    publishes.sort();
    publishes.dedup();
    publishes
}

/// Sealed sets are all or none: each one belongs to a Python publish of the intent and carries a wheel and an sdist.
fn validate_distributions(
    intent: &ReleaseIntentV1,
    entries: &[ArtifactManifestEntryV1],
    distributions: &[DistributionSetV1],
) -> Result<(), ArtifactManifestError> {
    if distributions.is_empty() {
        return Ok(());
    }
    if distributions.windows(2).any(|pair| pair[0].key() == pair[1].key()) {
        return Err(ArtifactManifestError::DuplicateDistributionSet);
    }
    let expected = python_publishes(intent);
    if distributions
        .iter()
        .map(|set| (set.package.clone(), set.version.clone()))
        .ne(expected
            .iter()
            .map(|(package, version)| ((*package).clone(), version.clone())))
    {
        return Err(ArtifactManifestError::MismatchedDistributionRoster);
    }
    let mut names: Vec<&str> = entries.iter().map(|entry| entry.slot.asset_name.as_str()).collect();
    for set in distributions {
        let files = || set.files.iter().map(|file| file.filename.as_str());
        if files().any(|name| !is_safe_artifact_component(name)) {
            return Err(ArtifactManifestError::UnsafeDistributionFile);
        }
        if !files().any(is_pypi_wheel) || !files().any(is_pypi_sdist) {
            return Err(ArtifactManifestError::IncompleteDistributionSet);
        }
        let owned = Version::parse(&set.version, VersionGrammar::Pep440)
            .is_ok_and(|version| files().all(|name| pypi_dist_belongs_to(name, set.package.name(), &version)));
        if !owned {
            return Err(ArtifactManifestError::ForeignDistributionFile);
        }
        names.extend(files());
    }
    names.sort_unstable();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ArtifactManifestError::DuplicateDistributionFile);
    }
    Ok(())
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
    #[error("artifact manifest repeats a Python file set")]
    DuplicateDistributionSet,
    #[error("artifact manifest Python file sets differ from the intent's Python publishes")]
    MismatchedDistributionRoster,
    #[error("a sealed Python file set needs both a wheel and an sdist")]
    IncompleteDistributionSet,
    #[error("a sealed Python file name must be a safe relative identifier")]
    UnsafeDistributionFile,
    #[error("a sealed Python file is not a distribution of its file set's package and version")]
    ForeignDistributionFile,
    #[error("artifact manifest names one file twice across its assets and Python file sets")]
    DuplicateDistributionFile,
}

#[cfg(test)]
mod tests {
    use crate::{CommitSha, Ecosystem};
    use std::cmp::Ordering;
    use std::collections::BTreeSet;

    use super::super::test_support::{native_intent_with_slot, native_versions, test_intent, test_intent_with_slots};
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

    fn python_intent(names: &[&str]) -> ReleaseIntentV1 {
        let operations = names
            .iter()
            .map(|name| {
                ReleaseOperation::registry_publish(
                    ReleasePackageId::new(Ecosystem::Pypi, name).unwrap(),
                    Version::parse_for_ecosystem("1.0.0", Ecosystem::Pypi).unwrap(),
                    super::super::test_support::registry_binding("pypi"),
                    vec![],
                )
                .unwrap()
            })
            .collect();
        test_intent_with_slots(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            operations,
            vec![],
        )
        .unwrap()
    }

    fn file(filename: &str) -> DistributionFileV1 {
        DistributionFileV1 {
            filename: filename.to_string(),
            digest: ArtifactDigest::from_bytes(filename.as_bytes()),
            byte_length: filename.len() as u64,
        }
    }

    fn set(name: &str, files: &[&str]) -> DistributionSetV1 {
        DistributionSetV1 {
            package: ReleasePackageId::new(Ecosystem::Pypi, name).unwrap(),
            version: "1.0.0".to_string(),
            files: files.iter().map(|name| file(name)).collect(),
        }
    }

    fn full_set(name: &str) -> DistributionSetV1 {
        set(
            name,
            &[
                &format!("{name}-1.0.0-py3-none-any.whl"),
                &format!("{name}-1.0.0.tar.gz"),
            ],
        )
    }

    #[test]
    fn a_manifest_without_python_sets_serializes_without_the_field() {
        let intent = native_intent_with_slot(Ecosystem::Cargo, "1.2.3");
        let manifest = ArtifactManifestV1::new(&intent, vec![]);
        // The slot roster is not empty, so an empty entry list is the mismatch that proves `new` still checks it.
        assert!(matches!(manifest, Err(ArtifactManifestError::MismatchedSlotRoster)));
        let intent = python_intent(&["demo"]);
        let manifest = ArtifactManifestV1::new(&intent, vec![]).unwrap();
        assert!(!serde_json::to_string(&manifest).unwrap().contains("distributions"));
        manifest.validate_for_intent(&intent).unwrap();
    }

    #[test]
    fn sealed_python_sets_round_trip_sorted_and_bind_to_the_intent() {
        let intent = python_intent(&["alpha", "beta"]);
        let manifest =
            ArtifactManifestV1::with_distributions(&intent, vec![], vec![full_set("beta"), full_set("alpha")]).unwrap();
        assert_eq!(manifest.distributions[0].package.to_string(), "pypi/alpha");
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let decoded: ArtifactManifestV1 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded, manifest);
        decoded.validate_for_intent(&intent).unwrap();
        assert_ne!(
            manifest.digest(),
            ArtifactManifestV1::new(&intent, vec![]).unwrap().digest()
        );
    }

    #[test]
    fn sealed_python_sets_must_cover_exactly_the_intent_python_publishes() {
        let intent = python_intent(&["alpha", "beta"]);
        let partial = ArtifactManifestV1::with_distributions(&intent, vec![], vec![full_set("alpha")]);
        assert!(matches!(
            partial,
            Err(ArtifactManifestError::MismatchedDistributionRoster)
        ));
        let stray = ArtifactManifestV1::with_distributions(
            &intent,
            vec![],
            vec![full_set("alpha"), full_set("beta"), full_set("gamma")],
        );
        assert!(matches!(
            stray,
            Err(ArtifactManifestError::MismatchedDistributionRoster)
        ));
        let mut other_version = full_set("beta");
        other_version.version = "2.0.0".to_string();
        let wrong = ArtifactManifestV1::with_distributions(&intent, vec![], vec![full_set("alpha"), other_version]);
        assert!(matches!(
            wrong,
            Err(ArtifactManifestError::MismatchedDistributionRoster)
        ));
    }

    #[test]
    fn a_sealed_python_set_is_rejected_when_malformed() {
        let intent = python_intent(&["demo"]);
        let build = |set: DistributionSetV1| ArtifactManifestV1::with_distributions(&intent, vec![], vec![set]);
        assert!(matches!(
            build(set("demo", &["demo-1.0.0.tar.gz"])),
            Err(ArtifactManifestError::IncompleteDistributionSet)
        ));
        assert!(matches!(
            build(set("demo", &["demo-1.0.0-py3-none-any.whl"])),
            Err(ArtifactManifestError::IncompleteDistributionSet)
        ));
        assert!(matches!(
            build(set("demo", &["../demo-1.0.0-py3-none-any.whl", "demo-1.0.0.tar.gz"])),
            Err(ArtifactManifestError::UnsafeDistributionFile)
        ));
        assert!(matches!(
            build(set(
                "demo",
                &["demo-1.0.0-py3-none-any.whl", "demo-1.0.0.tar.gz", "demo-1.0.0.tar.gz"]
            )),
            Err(ArtifactManifestError::DuplicateDistributionFile)
        ));
        assert!(matches!(
            build(set("demo", &["other-1.0.0-py3-none-any.whl", "demo-1.0.0.tar.gz"])),
            Err(ArtifactManifestError::ForeignDistributionFile)
        ));
        assert!(matches!(
            build(set("demo", &["demo-1.0.1-py3-none-any.whl", "demo-1.0.0.tar.gz"])),
            Err(ArtifactManifestError::ForeignDistributionFile)
        ));
        let intent = python_intent(&["demo"]);
        let twice = ArtifactManifestV1::with_distributions(&intent, vec![], vec![full_set("demo"), full_set("demo")]);
        assert!(matches!(twice, Err(ArtifactManifestError::DuplicateDistributionSet)));
    }

    #[test]
    fn a_python_only_intent_accepts_a_manifest_digest_in_its_envelope() {
        let intent = python_intent(&["demo"]);
        let manifest = ArtifactManifestV1::with_distributions(&intent, vec![], vec![full_set("demo")]).unwrap();
        let orchestration = CommitSha::parse(&"b".repeat(40)).unwrap();
        ReleaseRunEnvelopeV1::new(orchestration.clone(), &intent, Some(manifest.digest())).unwrap();
        ReleaseRunEnvelopeV1::new(orchestration.clone(), &intent, None).unwrap();
        let cargo = test_intent(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            vec![ReleaseOperation::registry_publish(
                ReleasePackageId::new(Ecosystem::Cargo, "demo").unwrap(),
                Version::semver(1, 0, 0),
                super::super::test_support::registry_binding("crates"),
                vec![],
            )
            .unwrap()],
        )
        .unwrap();
        assert!(matches!(
            ReleaseRunEnvelopeV1::new(orchestration, &cargo, Some(manifest.digest())),
            Err(ReleaseRunEnvelopeError::UnexpectedArtifactManifest)
        ));
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
