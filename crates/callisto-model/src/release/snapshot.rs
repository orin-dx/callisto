use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{CanonicalTranscript, ReleasePackageId, SemanticInputDigest};
use crate::CommitSha;

/// The durable source identity used to bind an approved release intent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum SourceIdentity {
    GitCommit { sha: CommitSha },
    HermeticContent { digest: SemanticInputDigest },
}

impl SourceIdentity {
    pub fn git_commit(raw_sha: impl AsRef<str>) -> Result<Self, crate::ModelError> {
        Ok(Self::GitCommit {
            sha: CommitSha::parse(raw_sha.as_ref())?,
        })
    }
}

/// The execution trust model selected for an intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionTrustProfileV1 {
    GitCommit,
    HermeticContent,
}

/// A closed semantic projection for one exact package in a release intent.
///
/// The model never accepts caller-defined component names: adding a durable
/// input must add a typed field and update the transcript below. This makes
/// the schema itself the inventory of release-authorizing inputs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleasePackageInputV1 {
    pub package: ReleasePackageId,
    pub fingerprint: SemanticInputDigest,
}

/// The versioned semantic input projection built and compared by callisto-graph.
///
/// This model deliberately stores only typed, already-derived fingerprints.
/// Graph owns the projection and never puts raw endpoints, credentials,
/// command lines, or environment values here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseInputSnapshotV1 {
    pub schema_version: u8,
    pub source: SourceIdentity,
    pub packages: Vec<ReleasePackageInputV1>,
}

impl ReleaseInputSnapshotV1 {
    pub const SCHEMA_VERSION: u8 = 1;

    pub fn new(
        source: SourceIdentity,
        mut packages: Vec<ReleasePackageInputV1>,
    ) -> Result<Self, ReleaseInputSnapshotError> {
        packages.sort();
        if packages.windows(2).any(|pair| pair[0].package == pair[1].package) {
            return Err(ReleaseInputSnapshotError::DuplicatePackage);
        }
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            source,
            packages,
        })
    }

    pub fn digest(&self) -> SemanticInputDigest {
        let mut transcript = CanonicalTranscript::semantic_input_v1();
        transcript.push_bytes("schema", [self.schema_version]);
        match &self.source {
            SourceIdentity::GitCommit { sha } => transcript.push_str("source.git", sha.as_str()),
            SourceIdentity::HermeticContent { digest } => transcript.push_str("source.hermetic", digest.as_str()),
        }
        for package in &self.packages {
            transcript.push_str("package", &package.package.to_string());
            transcript.push_str("package.fingerprint", package.fingerprint.as_str());
        }
        SemanticInputDigest::from_transcript(&transcript)
    }
}

/// Errors in a typed semantic input snapshot.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReleaseInputSnapshotError {
    #[error("release input snapshot repeats a package")]
    DuplicatePackage,
}
