//! Typed, provider-neutral decisions for a managed release pull request.
//!
//! This is deliberately pre-merge collaboration policy, rather than durable
//! release authority. The values name a narrow, credential-free observation
//! and the one forge mutation sequence that an executor may perform.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::CommitSha;

/// Explicit identity and presentation configuration for one managed release PR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleasePrConfigV1 {
    pub schema_version: u8,
    pub repository: String,
    pub base_branch: String,
    pub release_branch: String,
}

impl ReleasePrConfigV1 {
    pub const SCHEMA_VERSION: u8 = 1;

    pub fn new(
        repository: String,
        base_branch: String,
        release_branch: String,
    ) -> Result<Self, ReleasePrDecisionError> {
        validate_repository(&repository)?;
        validate_branch("base branch", &base_branch)?;
        validate_branch("managed release branch", &release_branch)?;
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            repository,
            base_branch,
            release_branch,
        })
    }
}

impl<'de> Deserialize<'de> for ReleasePrConfigV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Wire {
            schema_version: u8,
            repository: String,
            base_branch: String,
            release_branch: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        if wire.schema_version != Self::SCHEMA_VERSION {
            return Err(serde::de::Error::custom(
                "unsupported release PR configuration schema version",
            ));
        }
        Self::new(wire.repository, wire.base_branch, wire.release_branch).map_err(serde::de::Error::custom)
    }
}

/// A credential-free observation supplied by a forge adapter before planning.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleasePrSnapshotV1 {
    pub schema_version: u8,
    pub repository: String,
    pub base_branch: String,
    pub base_commit: CommitSha,
    pub open_pull_requests: Vec<ReleasePrPullRequestV1>,
}

impl ReleasePrSnapshotV1 {
    pub const SCHEMA_VERSION: u8 = 1;

    pub fn new(
        repository: String,
        base_branch: String,
        base_commit: CommitSha,
        mut open_pull_requests: Vec<ReleasePrPullRequestV1>,
    ) -> Result<Self, ReleasePrDecisionError> {
        validate_repository(&repository)?;
        validate_branch("base branch", &base_branch)?;
        let mut numbers = open_pull_requests.iter().map(|pr| pr.number).collect::<Vec<_>>();
        numbers.sort_unstable();
        if numbers.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ReleasePrDecisionError::DuplicatePullRequestNumber);
        }
        open_pull_requests.sort_by_key(|pr| pr.number);
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            repository,
            base_branch,
            base_commit,
            open_pull_requests,
        })
    }
}

impl<'de> Deserialize<'de> for ReleasePrSnapshotV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Wire {
            schema_version: u8,
            repository: String,
            base_branch: String,
            base_commit: CommitSha,
            open_pull_requests: Vec<ReleasePrPullRequestV1>,
        }
        let wire = Wire::deserialize(deserializer)?;
        if wire.schema_version != Self::SCHEMA_VERSION {
            return Err(serde::de::Error::custom(
                "unsupported release PR snapshot schema version",
            ));
        }
        Self::new(
            wire.repository,
            wire.base_branch,
            wire.base_commit,
            wire.open_pull_requests,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// One open pull request observed by a forge adapter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleasePrPullRequestV1 {
    pub number: u64,
    pub head_repository: String,
    pub head_branch: String,
    /// A Git observation, not a policy choice: true exactly when this branch
    /// differs from the current base at a workflow path.
    pub workflow_delta_from_base: bool,
}

/// A closed release-PR operation selected by Callisto.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
#[non_exhaustive]
pub enum ReleasePrActionV1 {
    Noop {
        reason: ReleasePrNoopReasonV1,
    },
    Create {
        branch: String,
    },
    Update {
        pull_request_number: u64,
        branch: String,
    },
    Supersede {
        pull_request_number: u64,
        expected_branch: String,
        replacement_branch: String,
    },
}

/// Why a release-PR run is intentionally mutation-free.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
#[non_exhaustive]
pub enum ReleasePrNoopReasonV1 {
    NoPendingChangesets,
}

/// Versioned output of managed release-PR policy derivation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleasePrDecisionV1 {
    pub schema_version: u8,
    pub config: ReleasePrConfigV1,
    pub snapshot: ReleasePrSnapshotV1,
    pub action: ReleasePrActionV1,
}

impl ReleasePrDecisionV1 {
    pub const SCHEMA_VERSION: u8 = 1;

    /// Derives the only permitted action from fresh workspace and forge facts.
    pub fn derive(
        has_pending_changesets: bool,
        config: &ReleasePrConfigV1,
        snapshot: &ReleasePrSnapshotV1,
    ) -> Result<Self, ReleasePrDecisionError> {
        if snapshot.repository != config.repository || snapshot.base_branch != config.base_branch {
            return Err(ReleasePrDecisionError::SnapshotIdentityMismatch);
        }

        let mut eligible = Vec::new();
        for pr in &snapshot.open_pull_requests {
            let shape = managed_branch_shape(&config.release_branch, &pr.head_branch)?;
            if shape.is_some() {
                if pr.head_repository != config.repository {
                    return Err(ReleasePrDecisionError::ForeignManagedPullRequest {
                        number: pr.number,
                        repository: pr.head_repository.clone(),
                    });
                }
                eligible.push(pr);
            }
        }
        if eligible.len() > 1 {
            return Err(ReleasePrDecisionError::AmbiguousManagedPullRequests { count: eligible.len() });
        }

        let action = if !has_pending_changesets {
            ReleasePrActionV1::Noop {
                reason: ReleasePrNoopReasonV1::NoPendingChangesets,
            }
        } else if let Some(pr) = eligible.first() {
            if pr.workflow_delta_from_base {
                ReleasePrActionV1::Supersede {
                    pull_request_number: pr.number,
                    expected_branch: pr.head_branch.clone(),
                    replacement_branch: format!("{}--{}", config.release_branch, snapshot.base_commit.as_str()),
                }
            } else {
                ReleasePrActionV1::Update {
                    pull_request_number: pr.number,
                    branch: pr.head_branch.clone(),
                }
            }
        } else {
            ReleasePrActionV1::Create {
                branch: config.release_branch.clone(),
            }
        };
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            config: config.clone(),
            snapshot: snapshot.clone(),
            action,
        })
    }

    /// Rejects any post-decision forge state change before the executor mutates.
    pub fn verify_snapshot(&self, observed: &ReleasePrSnapshotV1) -> Result<(), ReleasePrDecisionError> {
        if observed != &self.snapshot {
            return Err(ReleasePrDecisionError::SnapshotChanged);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for ReleasePrDecisionV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Wire {
            schema_version: u8,
            config: ReleasePrConfigV1,
            snapshot: ReleasePrSnapshotV1,
            action: ReleasePrActionV1,
        }
        let wire = Wire::deserialize(deserializer)?;
        if wire.schema_version != Self::SCHEMA_VERSION {
            return Err(serde::de::Error::custom(
                "unsupported release PR decision schema version",
            ));
        }
        Ok(Self {
            schema_version: wire.schema_version,
            config: wire.config,
            snapshot: wire.snapshot,
            action: wire.action,
        })
    }
}

/// Invalid or unsafe release-PR observations and policies.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, miette::Diagnostic)]
#[non_exhaustive]
pub enum ReleasePrDecisionError {
    #[error("release PR repository `{repository}` is invalid")]
    #[diagnostic(
        code(E141),
        help("Use the exact GitHub owner/repository identity, for example `orin-dx/callisto`.")
    )]
    InvalidRepository { repository: String },
    #[error("release PR {kind} `{branch}` is not a safe Git branch name")]
    #[diagnostic(code(E142), help("Use a non-empty Git ref name without whitespace, control characters, `..`, `@{{`, or shell-sensitive prefixes."))]
    InvalidBranch { kind: &'static str, branch: String },
    #[error("release PR snapshot contains the same pull request number more than once")]
    #[diagnostic(
        code(E143),
        help("Refresh the forge snapshot and pass each open pull request exactly once.")
    )]
    DuplicatePullRequestNumber,
    #[error("release PR snapshot does not match the configured repository or base branch")]
    #[diagnostic(
        code(E144),
        help(
            "Collect the snapshot for the configured repository and base branch immediately before calling Callisto."
        )
    )]
    SnapshotIdentityMismatch,
    #[error("managed release PR #{number} comes from foreign repository `{repository}`")]
    #[diagnostic(
        code(E145),
        help("Do not treat a fork branch as Callisto-managed; close or rename the lookalike before retrying.")
    )]
    ForeignManagedPullRequest { number: u64, repository: String },
    #[error("found {count} open managed release PRs")]
    #[diagnostic(
        code(E146),
        help("Resolve the ambiguous managed PRs manually before running the release action again.")
    )]
    AmbiguousManagedPullRequests { count: usize },
    #[error("managed release branch `{branch}` has an invalid SHA suffix")]
    #[diagnostic(
        code(E147),
        help(
            "Use the canonical branch or the canonical branch followed by `--` and one lowercase forty-hex commit SHA."
        )
    )]
    MalformedManagedBranch { branch: String },
    #[error("release PR snapshot changed after Callisto made its decision")]
    #[diagnostic(
        code(E148),
        help("Re-run `callisto release-pr decide` with a fresh snapshot; no forge mutation was made.")
    )]
    SnapshotChanged,
}

fn validate_repository(repository: &str) -> Result<(), ReleasePrDecisionError> {
    let mut parts = repository.split('/');
    let valid = matches!((parts.next(), parts.next(), parts.next()), (Some(owner), Some(name), None) if valid_repo_part(owner) && valid_repo_part(name));
    if valid {
        Ok(())
    } else {
        Err(ReleasePrDecisionError::InvalidRepository {
            repository: repository.to_string(),
        })
    }
}

fn valid_repo_part(part: &str) -> bool {
    !part.is_empty()
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn validate_branch(kind: &'static str, branch: &str) -> Result<(), ReleasePrDecisionError> {
    let valid = !branch.is_empty()
        && !branch.starts_with('-')
        && !branch.starts_with('/')
        && !branch.ends_with('/')
        && !branch.contains("//")
        && !branch.contains("..")
        && !branch.contains("@{")
        && branch != "@"
        && branch
            .split('/')
            .all(|part| !part.starts_with('.') && !part.ends_with(".lock"))
        && branch.chars().all(|ch| {
            !ch.is_ascii_control() && !ch.is_whitespace() && !matches!(ch, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        });
    if valid {
        Ok(())
    } else {
        Err(ReleasePrDecisionError::InvalidBranch {
            kind,
            branch: branch.to_string(),
        })
    }
}

fn managed_branch_shape(canonical: &str, branch: &str) -> Result<Option<()>, ReleasePrDecisionError> {
    if branch == canonical {
        return Ok(Some(()));
    }
    let Some(suffix) = branch.strip_prefix(&format!("{canonical}--")) else {
        return Ok(None);
    };
    if suffix.len() == 40
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(Some(()))
    } else {
        Err(ReleasePrDecisionError::MalformedManagedBranch {
            branch: branch.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha() -> CommitSha {
        CommitSha::parse("0123456789abcdef0123456789abcdef01234567").unwrap()
    }

    fn config() -> ReleasePrConfigV1 {
        ReleasePrConfigV1::new(
            "orin-dx/callisto".to_string(),
            "main".to_string(),
            "callisto/version-packages".to_string(),
        )
        .unwrap()
    }

    fn snapshot(prs: Vec<ReleasePrPullRequestV1>) -> ReleasePrSnapshotV1 {
        ReleasePrSnapshotV1::new("orin-dx/callisto".to_string(), "main".to_string(), sha(), prs).unwrap()
    }

    fn pr(number: u64, branch: &str, workflow_delta_from_base: bool) -> ReleasePrPullRequestV1 {
        ReleasePrPullRequestV1 {
            number,
            head_repository: "orin-dx/callisto".to_string(),
            head_branch: branch.to_string(),
            workflow_delta_from_base,
        }
    }

    #[test]
    fn derives_create_when_no_managed_pr_exists() {
        let decision = ReleasePrDecisionV1::derive(true, &config(), &snapshot(vec![])).unwrap();
        assert_eq!(
            decision.action,
            ReleasePrActionV1::Create {
                branch: "callisto/version-packages".to_string()
            }
        );
        assert_eq!(decision.config, config());
    }

    #[test]
    fn derives_update_for_one_canonical_or_replacement_pr() {
        for branch in [
            "callisto/version-packages",
            "callisto/version-packages--0123456789abcdef0123456789abcdef01234567",
        ] {
            let decision =
                ReleasePrDecisionV1::derive(true, &config(), &snapshot(vec![pr(42, branch, false)])).unwrap();
            assert_eq!(
                decision.action,
                ReleasePrActionV1::Update {
                    pull_request_number: 42,
                    branch: branch.to_string()
                }
            );
        }
    }

    #[test]
    fn derives_deterministic_supersede_when_workflow_delta_is_observed() {
        let decision = ReleasePrDecisionV1::derive(
            true,
            &config(),
            &snapshot(vec![pr(42, "callisto/version-packages", true)]),
        )
        .unwrap();
        assert_eq!(
            decision.action,
            ReleasePrActionV1::Supersede {
                pull_request_number: 42,
                expected_branch: "callisto/version-packages".to_string(),
                replacement_branch: "callisto/version-packages--0123456789abcdef0123456789abcdef01234567".to_string(),
            }
        );
    }

    #[test]
    fn no_pending_changesets_is_noop_after_snapshot_validation() {
        let decision = ReleasePrDecisionV1::derive(
            false,
            &config(),
            &snapshot(vec![pr(42, "callisto/version-packages", true)]),
        )
        .unwrap();
        assert_eq!(
            decision.action,
            ReleasePrActionV1::Noop {
                reason: ReleasePrNoopReasonV1::NoPendingChangesets
            }
        );
    }

    #[test]
    fn rejects_ambiguous_foreign_and_malformed_managed_prs() {
        let ambiguous = snapshot(vec![
            pr(1, "callisto/version-packages", false),
            pr(
                2,
                "callisto/version-packages--0123456789abcdef0123456789abcdef01234567",
                false,
            ),
        ]);
        assert!(matches!(
            ReleasePrDecisionV1::derive(true, &config(), &ambiguous),
            Err(ReleasePrDecisionError::AmbiguousManagedPullRequests { .. })
        ));

        let mut foreign = pr(1, "callisto/version-packages", false);
        foreign.head_repository = "fork/callisto".to_string();
        assert!(matches!(
            ReleasePrDecisionV1::derive(true, &config(), &snapshot(vec![foreign])),
            Err(ReleasePrDecisionError::ForeignManagedPullRequest { .. })
        ));

        assert!(matches!(
            ReleasePrDecisionV1::derive(
                true,
                &config(),
                &snapshot(vec![pr(1, "callisto/version-packages--bad", false)])
            ),
            Err(ReleasePrDecisionError::MalformedManagedBranch { .. })
        ));
    }

    #[test]
    fn deserialization_rejects_unknown_fields_and_schema_mismatch() {
        let unknown = r#"{\"schemaVersion\":1,\"repository\":\"orin-dx/callisto\",\"baseBranch\":\"main\",\"releaseBranch\":\"callisto/version-packages\",\"unexpected\":true}"#;
        assert!(serde_json::from_str::<ReleasePrConfigV1>(unknown).is_err());
        let version = r#"{\"schemaVersion\":2,\"repository\":\"orin-dx/callisto\",\"baseBranch\":\"main\",\"releaseBranch\":\"callisto/version-packages\"}"#;
        assert!(serde_json::from_str::<ReleasePrConfigV1>(version).is_err());
    }

    #[test]
    fn snapshot_recheck_is_order_independent_but_rejects_a_race() {
        let decision = ReleasePrDecisionV1::derive(
            true,
            &config(),
            &snapshot(vec![pr(42, "callisto/version-packages", false)]),
        )
        .unwrap();
        let same = snapshot(vec![pr(42, "callisto/version-packages", false)]);
        assert!(decision.verify_snapshot(&same).is_ok());
        let changed = snapshot(vec![pr(43, "callisto/version-packages", false)]);
        assert!(matches!(
            decision.verify_snapshot(&changed),
            Err(ReleasePrDecisionError::SnapshotChanged)
        ));
    }
}
