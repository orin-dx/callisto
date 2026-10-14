use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use super::{CanonicalTranscript, DecisionDigest, ReleasePackageId};
use crate::{Version, VersionParseError};

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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "ReleaseDecisionEntryWire", rename_all = "camelCase", deny_unknown_fields)]
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

impl TryFrom<ReleaseDecisionEntryWire> for ReleaseDecisionEntry {
    type Error = VersionParseError;

    fn try_from(wire: ReleaseDecisionEntryWire) -> Result<Self, Self::Error> {
        let target_version = Version::parse_for_ecosystem(&wire.target_version, wire.package.ecosystem())?;
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

#[cfg(test)]
mod tests {
    use crate::Ecosystem;

    use super::super::test_support::native_versions;
    use super::*;
    use crate::release::*;

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
}
