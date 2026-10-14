use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use super::{CanonicalTranscript, DecisionDigest, ReleasePackageId};
use crate::Version;

/// The closed set of facts that can include a package in a durable release.
///
/// This is intentionally a model value: graph derives it, but every later
/// process can validate the exact approved roster without importing graph.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind",
    deny_unknown_fields
)]
#[non_exhaustive]
pub enum ReleaseInclusionReason {
    Changeset,
    Inference,
    ExplicitSelection,
    LinkedGroup {
        group_id: String,
    },
    FixedGroup {
        group_id: String,
    },
    Cascade {
        from: ReleasePackageId,
        edge_kind: String,
    },
    PreReleasePolicy {
        policy_id: String,
    },
    /// The package's current version has no tag yet.
    UnreleasedVersion,
}

/// One exact package and version authorized by a release decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseDecisionEntry {
    pub package: ReleasePackageId,
    pub target_version: Version,
    pub reasons: Vec<ReleaseInclusionReason>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseDecisionEntryWire {
    package: ReleasePackageId,
    target_version: String,
    reasons: Vec<ReleaseInclusionReason>,
}

impl<'de> Deserialize<'de> for ReleaseDecisionEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ReleaseDecisionEntryWire::deserialize(deserializer)?;
        let target_version = Version::parse_for_ecosystem(&wire.target_version, wire.package.ecosystem())
            .map_err(serde::de::Error::custom)?;
        Ok(Self {
            package: wire.package,
            target_version,
            reasons: wire.reasons,
        })
    }
}

/// Accepts every decision schema in [`ReleaseDecisionV1::READABLE_SCHEMA_VERSIONS`].
fn check_decision_schema_version<E: serde::de::Error>(found: u8, type_name: &str) -> Result<(), E> {
    if ReleaseDecisionV1::READABLE_SCHEMA_VERSIONS.contains(&found) {
        return Ok(());
    }
    Err(E::custom(format!(
        "unsupported {type_name} schema version {found}; this build reads versions {:?}",
        ReleaseDecisionV1::READABLE_SCHEMA_VERSIONS
    )))
}

/// Credential-free, deterministic release authority derived by callisto-graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseDecisionV1 {
    pub schema_version: u8,
    pub entries: Vec<ReleaseDecisionEntry>,
    pub digest: DecisionDigest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseDecisionV1Wire {
    schema_version: u8,
    entries: Vec<ReleaseDecisionEntry>,
    digest: DecisionDigest,
}

impl<'de> Deserialize<'de> for ReleaseDecisionV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ReleaseDecisionV1Wire::deserialize(deserializer)?;
        check_decision_schema_version::<D::Error>(wire.schema_version, "release decision")?;
        let decision = Self::new(wire.entries).map_err(serde::de::Error::custom)?;
        if decision.digest != wire.digest {
            return Err(serde::de::Error::custom(
                "release decision digest does not match canonical content",
            ));
        }
        Ok(decision)
    }
}

impl ReleaseDecisionV1 {
    pub const SCHEMA_VERSION: u8 = 2;
    /// Version 1 differs only in lacking `unreleasedVersion`, so a committed v1 decision still reads.
    pub const READABLE_SCHEMA_VERSIONS: [u8; 2] = [1, 2];

    /// Creates a canonical decision or rejects an ambiguous release roster.
    #[allow(clippy::result_large_err)]
    pub fn new(mut entries: Vec<ReleaseDecisionEntry>) -> Result<Self, ReleaseDecisionError> {
        if entries.is_empty() {
            return Err(ReleaseDecisionError::EmptyRoster);
        }
        for entry in &mut entries {
            entry.reasons.sort();
            entry.reasons.dedup();
            if entry.reasons.is_empty() {
                return Err(ReleaseDecisionError::MissingReason {
                    package: entry.package.clone(),
                });
            }
        }
        entries.sort_by(|left, right| left.package.cmp(&right.package));
        if entries.windows(2).any(|pair| pair[0].package == pair[1].package) {
            return Err(ReleaseDecisionError::DuplicatePackage);
        }

        // A fixed or linked group always converges every member to one
        // shared target version -- `solve_cascade` guarantees this when a
        // decision is derived from a version plan. Entries tag their own
        // group membership via `FixedGroup`/`LinkedGroup` reasons, so this
        // invariant is checkable from the decision's own content alone, with
        // no `GroupTable`/workspace-config access needed. Rejecting a
        // violation here means it also applies to a decision deserialized
        // from a committed release-decision file (`Deserialize` calls
        // `new()`), not just one freshly derived from a plan.
        let mut group_targets: BTreeMap<&str, &Version> = BTreeMap::new();
        for entry in &entries {
            for reason in &entry.reasons {
                let group_id = match reason {
                    ReleaseInclusionReason::FixedGroup { group_id }
                    | ReleaseInclusionReason::LinkedGroup { group_id } => group_id.as_str(),
                    _ => continue,
                };
                match group_targets.get(group_id) {
                    Some(existing) if **existing != entry.target_version => {
                        return Err(ReleaseDecisionError::DivergentGroupTarget {
                            group_id: group_id.to_string(),
                            left: (*existing).clone(),
                            right: entry.target_version.clone(),
                        });
                    }
                    _ => {
                        group_targets.insert(group_id, &entry.target_version);
                    }
                }
            }
        }

        let digest = decision_digest(&entries);
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            entries,
            digest,
        })
    }
}

pub(super) fn decision_digest(entries: &[ReleaseDecisionEntry]) -> DecisionDigest {
    let mut transcript = CanonicalTranscript::decision_v1();
    for entry in entries {
        transcript.push_str("package", &entry.package.to_string());
        transcript.push_str("target-version", entry.target_version.render());
        for reason in &entry.reasons {
            transcript.push_str(
                "reason",
                &serde_json::to_string(reason).expect("closed reason serializes"),
            );
        }
    }
    DecisionDigest::from_transcript(&transcript)
}

/// Decision construction or validation errors.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReleaseDecisionError {
    #[error("a durable release decision must contain at least one package")]
    EmptyRoster,
    #[error("release decision repeats a package")]
    DuplicatePackage,
    #[error("release decision entry for {package} has no inclusion reason")]
    MissingReason { package: ReleasePackageId },
    #[error("release decision claims divergent target versions for group `{group_id}`: {left} vs {right}")]
    DivergentGroupTarget {
        group_id: String,
        left: Version,
        right: Version,
    },
}
