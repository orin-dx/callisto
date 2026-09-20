//! Evidence-carrying provider observations.
//!
//! A bare four-state observation let "the object exists" stand in for "the
//! object is exactly what this intent authorized", so a receipt's observation
//! map proved nothing. Every non-absent state here carries the closed,
//! credential-free proof the provider actually produced, and the only tokens
//! that authorize a state transition are minted from one of these states.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::release::{ReleaseOperationId, ReleaseOperationRole};
use crate::tag::TagName;
use crate::{ArtifactDigest, CommitSha, Version};

/// What a provider proved about one operation's remote identity.
///
/// Closed and per-role: an adapter may only report what its provider can
/// demonstrate today. Fields that a provider cannot yet report are `None`
/// rather than invented.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum ProviderEvidenceV1 {
    /// The registry served this exact version.
    RegistryVersion {
        version: Version,
        /// `None` until the configured client reports a package checksum.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checksum: Option<ArtifactDigest>,
        /// `None` until the configured client reports yank status.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        yanked: Option<bool>,
    },
    /// An annotated tag on the release remote, resolved to its commit.
    GitTag {
        peeled_commit: CommitSha,
    },
    ForgeRelease {
        tag_name: TagName,
        draft: bool,
    },
    ArtifactUpload {
        byte_length: u64,
        sha256: ArtifactDigest,
    },
}

impl ProviderEvidenceV1 {
    fn matches_role(&self, role: &ReleaseOperationRole) -> bool {
        matches!(
            (self, role),
            (
                Self::RegistryVersion { .. },
                ReleaseOperationRole::RegistryPublish { .. }
            ) | (Self::GitTag { .. }, ReleaseOperationRole::Tag)
                | (Self::ForgeRelease { .. }, ReleaseOperationRole::ForgeRelease)
                | (Self::ArtifactUpload { .. }, ReleaseOperationRole::ArtifactUpload { .. })
        )
    }
}

/// Why an existing remote object is not the one this intent authorized.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ProviderConflictReason {
    /// A local tag ref disagrees with the prepared tag target or annotation.
    LocalTagDiffers,
    /// The remote tag resolves to a different commit.
    RemoteTagTargetDiffers,
    /// The remote tag is lightweight, so it carries no release annotation.
    UnannotatedTag,
    /// The forge release exists but is a draft or names another tag.
    ForgeReleaseDiffers,
    /// The release asset exists with different bytes or length.
    ArtifactAssetDiffers,
    /// Several assets share the slot's asset name.
    DuplicateArtifactAsset,
    /// The registry serves this version, but it is yanked.
    RegistryVersionYanked,
}

/// Why remote identity could not be established at all.
///
/// Indeterminate is never a conflict: an authentication, rate-limit, or
/// transport failure must not be reported as a disagreeing remote object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
#[non_exhaustive]
pub enum ProviderIndeterminateCause {
    /// No query API exists for this provider yet (PyPI's upload endpoint).
    UnsupportedProvider,
    /// The provider answered with a status that proves neither presence nor absence.
    ProviderStatus {
        status: u16,
    },
    CommandFailed,
    MalformedResponse,
    Timeout,
    /// The version exists but its identity was not observed.
    RegistryVersionUnverified,
    /// Asset identity needs the verified artifact manifest, which was absent.
    ArtifactManifestUnavailable,
}

/// The result of observing the exact remote identity of a release operation.
///
/// No arbitrary provider response is persisted: every variant carries only
/// closed, credential-free, action-relevant data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum ProviderObservationV1 {
    Absent,
    Exact { evidence: ProviderEvidenceV1 },
    Conflict { reason: ProviderConflictReason },
    Indeterminate { cause: ProviderIndeterminateCause },
}

impl ProviderObservationV1 {
    pub fn is_terminal_success(&self) -> bool {
        matches!(self, Self::Exact { .. })
    }

    /// Mints the proof that no effect has happened yet. This is the only way
    /// to obtain an [`AbsentProof`], so no code path can mark an operation
    /// `Attempting` without having observed its absence first.
    pub fn absent_proof(&self) -> Option<AbsentProof> {
        matches!(self, Self::Absent).then_some(AbsentProof(()))
    }

    /// Mints the proof that the provider holds exactly this operation's result.
    pub fn exact_evidence(&self) -> Option<ExactEvidence> {
        match self {
            Self::Exact { evidence } => Some(ExactEvidence(evidence.clone())),
            _ => None,
        }
    }
}

/// Proof, obtainable only from [`ProviderObservationV1::Absent`], that a
/// provider was queried and held nothing for this operation.
#[derive(Debug)]
pub struct AbsentProof(());

/// Proof, obtainable only from [`ProviderObservationV1::Exact`], carrying the
/// evidence the provider produced.
#[derive(Debug)]
pub struct ExactEvidence(ProviderEvidenceV1);

impl ExactEvidence {
    pub fn evidence(&self) -> &ProviderEvidenceV1 {
        &self.0
    }

    pub fn into_evidence(self) -> ProviderEvidenceV1 {
        self.0
    }
}

/// One provider observation bound to an exact operation in an immutable intent.
///
/// The single constructor rejects evidence whose shape does not belong to the
/// operation's role, so a forge-release proof can never satisfy a registry
/// publish. Deserialization goes through the same constructor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseOperationObservationV1 {
    operation: ReleaseOperationId,
    observation: ProviderObservationV1,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseOperationObservationV1Wire {
    operation: ReleaseOperationId,
    observation: ProviderObservationV1,
}

impl<'de> Deserialize<'de> for ReleaseOperationObservationV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ReleaseOperationObservationV1Wire::deserialize(deserializer)?;
        Self::new(wire.operation, wire.observation).map_err(serde::de::Error::custom)
    }
}

impl ReleaseOperationObservationV1 {
    pub fn new(
        operation: ReleaseOperationId,
        observation: ProviderObservationV1,
    ) -> Result<Self, ProviderObservationError> {
        if let ProviderObservationV1::Exact { evidence } = &observation {
            if !evidence.matches_role(&operation.role) {
                return Err(ProviderObservationError::EvidenceRoleMismatch {
                    id: Box::new(operation),
                });
            }
        }
        Ok(Self { operation, observation })
    }

    pub fn operation(&self) -> &ReleaseOperationId {
        &self.operation
    }

    pub fn observation(&self) -> &ProviderObservationV1 {
        &self.observation
    }

    pub fn into_parts(self) -> (ReleaseOperationId, ProviderObservationV1) {
        (self.operation, self.observation)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderObservationError {
    #[error("provider evidence does not belong to the role of release operation `{id:?}`")]
    EvidenceRoleMismatch { id: Box<ReleaseOperationId> },
}
