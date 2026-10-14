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
