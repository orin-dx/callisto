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
use crate::{ArtifactDigest, CommitSha, Ecosystem, Version, VersionParseError};

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

/// Why one piece of evidence cannot stand for one operation's role.
enum EvidenceRejection {
    Role,
    DraftRelease,
}

impl ProviderEvidenceV1 {
    /// Both forge roles are proved by the same GitHub Release object; only the
    /// `draft` flag separates "the draft exists" from "the release is published".
    fn check_role(&self, role: &ReleaseOperationRole) -> Result<(), EvidenceRejection> {
        match (self, role) {
            (Self::ForgeRelease { draft, .. }, ReleaseOperationRole::ForgePublish) => {
                (!draft).then_some(()).ok_or(EvidenceRejection::DraftRelease)
            }
            (Self::RegistryVersion { .. }, ReleaseOperationRole::RegistryPublish { .. })
            | (Self::RegistryVersion { .. }, ReleaseOperationRole::PlatformPublish { .. })
            | (Self::GitTag { .. }, ReleaseOperationRole::Tag)
            | (Self::ForgeRelease { .. }, ReleaseOperationRole::ForgeRelease)
            | (Self::ArtifactUpload { .. }, ReleaseOperationRole::ArtifactUpload { .. }) => Ok(()),
            _ => Err(EvidenceRejection::Role),
        }
    }
}

/// Why an existing remote object is not the one this intent authorized.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ProviderConflictReason {
    /// A local tag ref names another commit, or is lightweight.
    LocalTagDiffers,
    /// The remote tag resolves to a different commit.
    RemoteTagTargetDiffers,
    /// The remote tag is lightweight, so it carries no release annotation.
    UnannotatedTag,
    /// The forge release exists but names another tag.
    ForgeReleaseDiffers,
    /// A release for this tag exists, but its `prerelease` flag disagrees with
    /// the released version's semver pre-release part.
    ForgeReleasePrereleaseDiffers,
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
    /// No client for this provider can prove a version present or absent.
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

impl std::fmt::Display for ProviderIndeterminateCause {
    /// Fixed text only, so it is safe to show an operator.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedProvider => formatter.write_str("no client can observe this provider"),
            Self::ProviderStatus { status } => {
                write!(
                    formatter,
                    "the provider answered HTTP {status}, which proves neither presence nor absence"
                )
            }
            Self::CommandFailed => formatter.write_str("the observing command failed"),
            Self::MalformedResponse => formatter.write_str("the provider's answer could not be understood"),
            Self::Timeout => formatter.write_str("the observation timed out"),
            Self::RegistryVersionUnverified => {
                formatter.write_str("the version exists but its identity was not observed")
            }
            Self::ArtifactManifestUnavailable => {
                formatter.write_str("the asset's identity needs the verified artifact manifest, which was absent")
            }
        }
    }
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
    observation: ProviderObservationV1Wire,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
enum ProviderObservationV1Wire {
    Absent,
    Exact { evidence: ProviderEvidenceV1Wire },
    Conflict { reason: ProviderConflictReason },
    Indeterminate { cause: ProviderIndeterminateCause },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
enum ProviderEvidenceV1Wire {
    RegistryVersion {
        version: String,
        #[serde(default)]
        checksum: Option<ArtifactDigest>,
        #[serde(default)]
        yanked: Option<bool>,
    },
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

impl ProviderObservationV1Wire {
    fn into_observation(self, ecosystem: Ecosystem) -> Result<ProviderObservationV1, VersionParseError> {
        Ok(match self {
            Self::Absent => ProviderObservationV1::Absent,
            Self::Exact { evidence } => ProviderObservationV1::Exact {
                evidence: match evidence {
                    ProviderEvidenceV1Wire::RegistryVersion {
                        version,
                        checksum,
                        yanked,
                    } => ProviderEvidenceV1::RegistryVersion {
                        version: Version::parse_for_ecosystem(&version, ecosystem)?,
                        checksum,
                        yanked,
                    },
                    ProviderEvidenceV1Wire::GitTag { peeled_commit } => ProviderEvidenceV1::GitTag { peeled_commit },
                    ProviderEvidenceV1Wire::ForgeRelease { tag_name, draft } => {
                        ProviderEvidenceV1::ForgeRelease { tag_name, draft }
                    }
                    ProviderEvidenceV1Wire::ArtifactUpload { byte_length, sha256 } => {
                        ProviderEvidenceV1::ArtifactUpload { byte_length, sha256 }
                    }
                },
            },
            Self::Conflict { reason } => ProviderObservationV1::Conflict { reason },
            Self::Indeterminate { cause } => ProviderObservationV1::Indeterminate { cause },
        })
    }
}

impl<'de> Deserialize<'de> for ReleaseOperationObservationV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ReleaseOperationObservationV1Wire::deserialize(deserializer)?;
        let observation = wire
            .observation
            .into_observation(wire.operation.package.ecosystem())
            .map_err(serde::de::Error::custom)?;
        Self::new(wire.operation, observation).map_err(serde::de::Error::custom)
    }
}

impl ReleaseOperationObservationV1 {
    pub fn new(
        operation: ReleaseOperationId,
        observation: ProviderObservationV1,
    ) -> Result<Self, ProviderObservationError> {
        if let ProviderObservationV1::Exact { evidence } = &observation {
            match evidence.check_role(&operation.role) {
                Ok(()) => {}
                Err(EvidenceRejection::Role) => {
                    return Err(ProviderObservationError::EvidenceRoleMismatch {
                        id: Box::new(operation),
                    })
                }
                Err(EvidenceRejection::DraftRelease) => {
                    return Err(ProviderObservationError::DraftReleaseCannotProvePublication {
                        id: Box::new(operation),
                    })
                }
            }
            if let ProviderEvidenceV1::RegistryVersion { version, .. } = evidence {
                if version != &operation.version {
                    return Err(ProviderObservationError::EvidenceVersionMismatch {
                        id: Box::new(operation),
                    });
                }
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
    #[error("a draft forge release cannot prove publication of release operation `{id:?}`")]
    DraftReleaseCannotProvePublication { id: Box<ReleaseOperationId> },
    #[error("provider version does not match release operation `{id:?}`")]
    EvidenceVersionMismatch { id: Box<ReleaseOperationId> },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RegistryBindingDigest, RegistryBindingId, ReleasePackageId, VersionGrammar};

    fn registry_operation(ecosystem: Ecosystem, raw: &str) -> ReleaseOperationId {
        ReleaseOperationId::registry_publish(
            ReleasePackageId::new(ecosystem, "example").unwrap(),
            Version::parse_for_ecosystem(raw, ecosystem).unwrap(),
            RegistryBindingId::new("native", RegistryBindingDigest::from_normalized_binding(b"native")).unwrap(),
        )
    }

    #[test]
    fn bound_registry_observations_restore_native_versions_and_optional_evidence() {
        for (ecosystem, raw) in [
            (Ecosystem::Pypi, "1.2.3"),
            (Ecosystem::Pypi, "1.2.3rc1"),
            (Ecosystem::Pypi, "1!2.3.post1.dev2"),
            (Ecosystem::Cargo, "1.2.3-beta.1+build.2"),
            (Ecosystem::Npm, "1.2.3-beta.1+build.2"),
        ] {
            let operation = registry_operation(ecosystem, raw);
            for (checksum, yanked) in [
                (None, None),
                (Some(ArtifactDigest::from_bytes(b"package")), Some(false)),
            ] {
                let observation = ReleaseOperationObservationV1::new(
                    operation.clone(),
                    ProviderObservationV1::Exact {
                        evidence: ProviderEvidenceV1::RegistryVersion {
                            version: operation.version.clone(),
                            checksum,
                            yanked,
                        },
                    },
                )
                .unwrap();
                let serialized = serde_json::to_vec(&observation).unwrap();
                let decoded: ReleaseOperationObservationV1 = serde_json::from_slice(&serialized).unwrap();
                assert_eq!(decoded, observation);
                assert_eq!(serde_json::to_vec(&decoded).unwrap(), serialized);
            }
        }
        let generic: ProviderEvidenceV1 =
            serde_json::from_str(r#"{"kind":"registryVersion","version":"1.2.3"}"#).unwrap();
        let ProviderEvidenceV1::RegistryVersion { version, .. } = generic else {
            panic!("expected registry evidence");
        };
        assert_eq!(version.grammar(), VersionGrammar::SemVer);
    }

    #[test]
    fn typed_registry_observations_reject_different_versions_and_grammars() {
        let operation = registry_operation(Ecosystem::Pypi, "1.2.3");
        for version in [
            Version::semver(1, 2, 3),
            Version::parse_for_ecosystem("1.2.4", Ecosystem::Pypi).unwrap(),
        ] {
            let observation = ProviderObservationV1::Exact {
                evidence: ProviderEvidenceV1::RegistryVersion {
                    version,
                    checksum: None,
                    yanked: None,
                },
            };
            assert!(matches!(
                ReleaseOperationObservationV1::new(operation.clone(), observation),
                Err(ProviderObservationError::EvidenceVersionMismatch { .. })
            ));
        }
    }

    #[test]
    fn bound_observation_wire_rejects_wrong_versions_cross_grammar_and_unknown_fields() {
        for (ecosystem, invalid) in [
            (Ecosystem::Pypi, "1.2.3-alpha.beta"),
            (Ecosystem::Cargo, "1.2.3a1"),
            (Ecosystem::Npm, "1!2.3rc1"),
        ] {
            let operation = registry_operation(ecosystem, "1.2.3");
            let wire = serde_json::json!({
                "operation": operation,
                "observation": { "kind": "exact", "evidence": { "kind": "registryVersion", "version": "1.2.3" } }
            });
            for value in [
                serde_json::json!(invalid),
                serde_json::json!("1.2.4"),
                serde_json::json!(123),
                serde_json::Value::Null,
            ] {
                let mut changed = wire.clone();
                changed["observation"]["evidence"]["version"] = value;
                assert!(serde_json::from_value::<ReleaseOperationObservationV1>(changed).is_err());
            }
            for field in ["extra", "grammar"] {
                let mut changed = wire.clone();
                changed["observation"]["evidence"][field] = serde_json::json!("pep440");
                assert!(serde_json::from_value::<ReleaseOperationObservationV1>(changed).is_err());
            }
            let mut changed = wire.clone();
            changed["extra"] = serde_json::json!(true);
            assert!(serde_json::from_value::<ReleaseOperationObservationV1>(changed).is_err());
            let mut changed = wire.clone();
            changed["observation"]["extra"] = serde_json::json!(true);
            assert!(serde_json::from_value::<ReleaseOperationObservationV1>(changed).is_err());
            let serialized = serde_json::to_string(&wire).unwrap();
            let duplicate = serialized.replacen(
                "\"kind\":\"registryVersion\",\"version\":\"1.2.3\"",
                "\"kind\":\"registryVersion\",\"version\":\"1.2.3\",\"version\":\"1.2.3\"",
                1,
            );
            assert_ne!(duplicate, serialized);
            assert!(serde_json::from_str::<ReleaseOperationObservationV1>(&duplicate).is_err());
        }
    }

    #[test]
    fn contextual_observation_wire_preserves_other_states_and_role_checks() {
        let operation = registry_operation(Ecosystem::Pypi, "1.2.3");
        for observation in [
            ProviderObservationV1::Absent,
            ProviderObservationV1::Conflict {
                reason: ProviderConflictReason::RegistryVersionYanked,
            },
            ProviderObservationV1::Indeterminate {
                cause: ProviderIndeterminateCause::ProviderStatus { status: 503 },
            },
        ] {
            let bound = ReleaseOperationObservationV1::new(operation.clone(), observation).unwrap();
            let decoded: ReleaseOperationObservationV1 =
                serde_json::from_slice(&serde_json::to_vec(&bound).unwrap()).unwrap();
            assert_eq!(decoded, bound);
        }
        let mismatched = serde_json::json!({
            "operation": operation,
            "observation": {
                "kind": "exact",
                "evidence": { "kind": "gitTag", "peeled_commit": "a".repeat(40) }
            }
        });
        let error = serde_json::from_value::<ReleaseOperationObservationV1>(mismatched).unwrap_err();
        assert!(error.to_string().contains("does not belong to the role"));
    }
}
