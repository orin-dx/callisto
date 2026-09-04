//! Typed, provider-neutral decisions for a managed release pull request.
//!
//! This is deliberately pre-merge collaboration policy, rather than durable
//! release authority. The values name a narrow, credential-free observation
//! and the one forge mutation sequence that an executor may perform.
//!
//! The executor never pushes Git objects directly: on a public repository,
//! GitHub's automatic `GITHUB_TOKEN` cannot write to `.github/workflows/*`
//! through the Git push protocol or through the forge commit API's own
//! `fileChanges`, on any ref. The managed branch is instead updated by
//! committing only non-workflow changes onto a deterministic staging branch
//! rooted at the current base commit (so it inherits the base's current
//! workflow content without writing to that path at all), then moving the
//! managed branch's ref onto that commit -- a ref update is not itself a
//! write to `.github/workflows/*` and is not subject to the restriction.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
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

    /// The deterministic staging branch this config's release branch commits
    /// through before any managed-branch ref is moved.
    pub fn staging_branch(&self) -> String {
        format!("{}--staging", self.release_branch)
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
pub struct ReleasePrSnapshotV2 {
    pub schema_version: u8,
    pub repository: String,
    pub base_branch: String,
    pub base_commit: CommitSha,
    pub open_pull_requests: Vec<ReleasePrPullRequestV2>,
}

impl ReleasePrSnapshotV2 {
    pub const SCHEMA_VERSION: u8 = 2;

    pub fn new(
        repository: String,
        base_branch: String,
        base_commit: CommitSha,
        mut open_pull_requests: Vec<ReleasePrPullRequestV2>,
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

impl<'de> Deserialize<'de> for ReleasePrSnapshotV2 {
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
            open_pull_requests: Vec<ReleasePrPullRequestV2>,
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
pub struct ReleasePrPullRequestV2 {
    pub number: u64,
    pub head_repository: String,
    pub head_branch: String,
    /// The pull request's current head commit. Re-observed and compared on
    /// every `verify_snapshot` call so a moved head is caught before the
    /// executor's non-compare-and-swap ref update.
    pub head_commit: CommitSha,
}

/// A closed release-PR operation selected by Callisto.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind",
    deny_unknown_fields
)]
#[non_exhaustive]
pub enum ReleasePrActionV2 {
    Noop {
        reason: ReleasePrNoopReasonV1,
    },
    Create {
        branch: String,
        staging_branch: String,
    },
    Update {
        pull_request_number: u64,
        branch: String,
        expected_head_commit: CommitSha,
        staging_branch: String,
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
pub struct ReleasePrDecisionV2 {
    pub schema_version: u8,
    pub config: ReleasePrConfigV1,
    pub snapshot: ReleasePrSnapshotV2,
    pub action: ReleasePrActionV2,
}

impl ReleasePrDecisionV2 {
    pub const SCHEMA_VERSION: u8 = 2;

    /// Derives the only permitted action from fresh workspace and forge facts.
    pub fn derive(
        has_pending_changesets: bool,
        config: &ReleasePrConfigV1,
        snapshot: &ReleasePrSnapshotV2,
    ) -> Result<Self, ReleasePrDecisionError> {
        if snapshot.repository != config.repository || snapshot.base_branch != config.base_branch {
            return Err(ReleasePrDecisionError::SnapshotIdentityMismatch);
        }

        let staging_branch = config.staging_branch();
        let mut eligible = Vec::new();
        for pr in &snapshot.open_pull_requests {
            if pr.head_branch == staging_branch {
                return Err(ReleasePrDecisionError::StagingBranchPullRequest { number: pr.number });
            }
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
            ReleasePrActionV2::Noop {
                reason: ReleasePrNoopReasonV1::NoPendingChangesets,
            }
        } else if let Some(pr) = eligible.first() {
            ReleasePrActionV2::Update {
                pull_request_number: pr.number,
                branch: pr.head_branch.clone(),
                expected_head_commit: pr.head_commit.clone(),
                staging_branch: staging_branch.clone(),
            }
        } else {
            ReleasePrActionV2::Create {
                branch: config.release_branch.clone(),
                staging_branch,
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
    pub fn verify_snapshot(&self, observed: &ReleasePrSnapshotV2) -> Result<(), ReleasePrDecisionError> {
        if observed != &self.snapshot {
            return Err(ReleasePrDecisionError::SnapshotChanged);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for ReleasePrDecisionV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Wire {
            schema_version: u8,
            config: ReleasePrConfigV1,
            snapshot: ReleasePrSnapshotV2,
            action: ReleasePrActionV2,
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
    #[error("commit plan for base commit `{base_commit}` has no file changes")]
    #[diagnostic(
        code(E149),
        help("An empty commit plan means nothing was staged; check the version command actually ran.")
    )]
    EmptyCommitPlan { base_commit: String },
    #[error("path `{path}` has unsupported Git mode `{mode:o}`")]
    #[diagnostic(
        code(E150),
        help("The forge commit API can only create plain, non-executable regular files; commit an executable bit, symlink, or submodule change through a privileged token instead.")
    )]
    UnsupportedFileMode { path: String, mode: u32 },
    #[error("path `{path}` has unsupported change kind `{kind:?}`")]
    #[diagnostic(
        code(E151),
        help("Renames, copies, type changes, and unmerged paths cannot be expressed as forge commit additions/deletions; stage a plain add/modify/delete instead.")
    )]
    UnsupportedChangeKind { path: String, kind: StagedChangeKindV1 },
    #[error("commit plan is {bytes} bytes, exceeding the {limit} byte limit")]
    #[diagnostic(
        code(E152),
        help("Split the release into smaller changesets, or reduce large generated files (for example a monolithic CHANGELOG) before retrying.")
    )]
    CommitPlanTooLarge { bytes: usize, limit: usize },
    #[error("path `{path}` may not appear in a forge commit plan")]
    #[diagnostic(
        code(E153),
        help("The executor never writes `.github/workflows/*`, `.git/*`, or unsafe paths through the forge commit API; those are inherited unchanged from the base commit.")
    )]
    ForbiddenCommitPlanPath { path: String },
    #[error("pull request #{number} targets the internal staging branch")]
    #[diagnostic(
        code(E154),
        help("The `<release-branch>--staging` branch is reserved for the executor's own commit staging; close or rename a pull request opened against it before retrying.")
    )]
    StagingBranchPullRequest { number: u64 },
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

/// The regular-file Git mode `createCommitOnBranch` can express. Any other
/// mode (executable, symlink, submodule) is refused by [`ReleasePrCommitPlanV1::from_changes`].
pub const REGULAR_FILE_MODE: u32 = 0o100644;

/// A conservative, unmeasured ceiling on total commit-plan content bytes. A
/// live measurement against GitHub's `createCommitOnBranch` size limit was
/// inconclusive (a client-side tooling failure in the measuring script, not
/// a server response); this stays comfortably below every documented and
/// observed limit while leaving ample headroom over today's real payload
/// (~150-200 KB). Revisit with a clean measurement if this is ever hit.
pub const MAX_COMMIT_PLAN_BYTES: usize = 4 * 1024 * 1024;

/// One file's staged change, as observed directly from the Git index. Not a
/// wire type: constructed by a VCS backend and consumed immediately by
/// [`ReleasePrCommitPlanV1::from_changes`] in the same process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedChangeV1 {
    pub path: String,
    pub kind: StagedChangeKindV1,
    /// The new Git file mode, for additions and modifications. `None` for a
    /// pure deletion.
    pub new_mode: Option<u32>,
    /// The new file's raw bytes, for additions and modifications. `None` for
    /// a pure deletion.
    pub contents: Option<Vec<u8>>,
}

/// The kind of change one path underwent relative to a base commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagedChangeKindV1 {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
}

/// One file addition in a [`ReleasePrCommitPlanV1`], mapped directly to a
/// GraphQL `FileAddition` (`path`, base64 `contents`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleasePrFileAdditionV1 {
    pub path: String,
    pub contents_base64: String,
}

/// One file deletion in a [`ReleasePrCommitPlanV1`], mapped directly to a
/// GraphQL `FileDeletion` (`path`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleasePrFileDeletionV1 {
    pub path: String,
}

/// A closed, typed description of the exact `createCommitOnBranch` file
/// changes the executor may submit. Never contains a `.github/workflows/*`
/// path: that content is always inherited unchanged from `base_commit`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleasePrCommitPlanV1 {
    pub schema_version: u8,
    pub base_commit: CommitSha,
    pub message: String,
    pub additions: Vec<ReleasePrFileAdditionV1>,
    pub deletions: Vec<ReleasePrFileDeletionV1>,
    pub total_content_bytes: usize,
}

impl ReleasePrCommitPlanV1 {
    pub const SCHEMA_VERSION: u8 = 1;

    /// Builds a commit plan from a Git index diff. Fails closed on anything
    /// the forge commit API cannot faithfully express or that must never
    /// cross this boundary: non-regular file modes, renames/copies/type
    /// changes/unmerged paths, `.github/workflows/*` (and other unsafe)
    /// paths, an empty change set, or a payload over [`MAX_COMMIT_PLAN_BYTES`].
    pub fn from_changes(
        base_commit: CommitSha,
        message: String,
        changes: Vec<StagedChangeV1>,
    ) -> Result<Self, ReleasePrDecisionError> {
        if changes.is_empty() {
            return Err(ReleasePrDecisionError::EmptyCommitPlan {
                base_commit: base_commit.as_str().to_string(),
            });
        }

        let mut additions = Vec::new();
        let mut deletions = Vec::new();
        let mut total_content_bytes = 0usize;

        for change in changes {
            validate_commit_plan_path(&change.path)?;
            match change.kind {
                StagedChangeKindV1::Added | StagedChangeKindV1::Modified => {
                    let mode = change.new_mode.unwrap_or(REGULAR_FILE_MODE);
                    if mode != REGULAR_FILE_MODE {
                        return Err(ReleasePrDecisionError::UnsupportedFileMode {
                            path: change.path,
                            mode,
                        });
                    }
                    let contents = change.contents.unwrap_or_default();
                    total_content_bytes += contents.len();
                    additions.push(ReleasePrFileAdditionV1 {
                        path: change.path,
                        contents_base64: BASE64.encode(contents),
                    });
                }
                StagedChangeKindV1::Deleted => {
                    deletions.push(ReleasePrFileDeletionV1 { path: change.path });
                }
                StagedChangeKindV1::Renamed
                | StagedChangeKindV1::Copied
                | StagedChangeKindV1::TypeChanged
                | StagedChangeKindV1::Unmerged => {
                    return Err(ReleasePrDecisionError::UnsupportedChangeKind {
                        path: change.path,
                        kind: change.kind,
                    });
                }
            }
        }

        if total_content_bytes > MAX_COMMIT_PLAN_BYTES {
            return Err(ReleasePrDecisionError::CommitPlanTooLarge {
                bytes: total_content_bytes,
                limit: MAX_COMMIT_PLAN_BYTES,
            });
        }

        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            base_commit,
            message,
            additions,
            deletions,
            total_content_bytes,
        })
    }
}

impl<'de> Deserialize<'de> for ReleasePrCommitPlanV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Wire {
            schema_version: u8,
            base_commit: CommitSha,
            message: String,
            additions: Vec<ReleasePrFileAdditionV1>,
            deletions: Vec<ReleasePrFileDeletionV1>,
            total_content_bytes: usize,
        }
        let wire = Wire::deserialize(deserializer)?;
        if wire.schema_version != Self::SCHEMA_VERSION {
            return Err(serde::de::Error::custom(
                "unsupported release PR commit plan schema version",
            ));
        }
        Ok(Self {
            schema_version: wire.schema_version,
            base_commit: wire.base_commit,
            message: wire.message,
            additions: wire.additions,
            deletions: wire.deletions,
            total_content_bytes: wire.total_content_bytes,
        })
    }
}

fn validate_commit_plan_path(path: &str) -> Result<(), ReleasePrDecisionError> {
    let forbidden = path.is_empty()
        || path.starts_with('/')
        || path.starts_with(".github/workflows/")
        || path == ".github/workflows"
        || path.starts_with(".git/")
        || path == ".git"
        || path.split('/').any(|part| part == "..")
        || path.contains('\0');
    if forbidden {
        Err(ReleasePrDecisionError::ForbiddenCommitPlanPath { path: path.to_string() })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha() -> CommitSha {
        CommitSha::parse("0123456789abcdef0123456789abcdef01234567").unwrap()
    }

    fn other_sha() -> CommitSha {
        CommitSha::parse("fedcba9876543210fedcba9876543210fedcba98").unwrap()
    }

    fn config() -> ReleasePrConfigV1 {
        ReleasePrConfigV1::new(
            "orin-dx/callisto".to_string(),
            "main".to_string(),
            "callisto/version-packages".to_string(),
        )
        .unwrap()
    }

    fn snapshot(prs: Vec<ReleasePrPullRequestV2>) -> ReleasePrSnapshotV2 {
        ReleasePrSnapshotV2::new("orin-dx/callisto".to_string(), "main".to_string(), sha(), prs).unwrap()
    }

    fn pr(number: u64, branch: &str) -> ReleasePrPullRequestV2 {
        ReleasePrPullRequestV2 {
            number,
            head_repository: "orin-dx/callisto".to_string(),
            head_branch: branch.to_string(),
            head_commit: other_sha(),
        }
    }

    #[test]
    fn derives_create_when_no_managed_pr_exists() {
        let decision = ReleasePrDecisionV2::derive(true, &config(), &snapshot(vec![])).unwrap();
        assert_eq!(
            decision.action,
            ReleasePrActionV2::Create {
                branch: "callisto/version-packages".to_string(),
                staging_branch: "callisto/version-packages--staging".to_string(),
            }
        );
        assert_eq!(decision.config, config());
    }

    #[test]
    fn derives_update_with_staging_branch_and_expected_head_for_canonical_or_replacement_pr() {
        for branch in [
            "callisto/version-packages",
            "callisto/version-packages--0123456789abcdef0123456789abcdef01234567",
        ] {
            let decision = ReleasePrDecisionV2::derive(true, &config(), &snapshot(vec![pr(42, branch)])).unwrap();
            assert_eq!(
                decision.action,
                ReleasePrActionV2::Update {
                    pull_request_number: 42,
                    branch: branch.to_string(),
                    expected_head_commit: other_sha(),
                    staging_branch: "callisto/version-packages--staging".to_string(),
                }
            );
        }
    }

    #[test]
    fn derive_rejects_pull_request_on_staging_branch() {
        let staging_pr = pr(7, "callisto/version-packages--staging");
        assert!(matches!(
            ReleasePrDecisionV2::derive(true, &config(), &snapshot(vec![staging_pr])),
            Err(ReleasePrDecisionError::StagingBranchPullRequest { number: 7 })
        ));
    }

    #[test]
    fn action_v2_fields_serialize_as_camel_case() {
        let update = ReleasePrActionV2::Update {
            pull_request_number: 42,
            branch: "callisto/version-packages".to_string(),
            expected_head_commit: other_sha(),
            staging_branch: "callisto/version-packages--staging".to_string(),
        };
        let value = serde_json::to_value(&update).unwrap();
        assert_eq!(value["kind"], "update");
        assert_eq!(value["pullRequestNumber"], 42);
        assert_eq!(value["expectedHeadCommit"], other_sha().as_str());
        assert_eq!(value["stagingBranch"], "callisto/version-packages--staging");
        assert!(value.get("pull_request_number").is_none());

        let create = ReleasePrActionV2::Create {
            branch: "callisto/version-packages".to_string(),
            staging_branch: "callisto/version-packages--staging".to_string(),
        };
        let value = serde_json::to_value(&create).unwrap();
        assert_eq!(value["kind"], "create");
        assert_eq!(value["stagingBranch"], "callisto/version-packages--staging");
    }

    #[test]
    fn no_pending_changesets_is_noop_after_snapshot_validation() {
        let decision =
            ReleasePrDecisionV2::derive(false, &config(), &snapshot(vec![pr(42, "callisto/version-packages")]))
                .unwrap();
        assert_eq!(
            decision.action,
            ReleasePrActionV2::Noop {
                reason: ReleasePrNoopReasonV1::NoPendingChangesets
            }
        );
    }

    #[test]
    fn rejects_ambiguous_foreign_and_malformed_managed_prs() {
        let ambiguous = snapshot(vec![
            pr(1, "callisto/version-packages"),
            pr(2, "callisto/version-packages--0123456789abcdef0123456789abcdef01234567"),
        ]);
        assert!(matches!(
            ReleasePrDecisionV2::derive(true, &config(), &ambiguous),
            Err(ReleasePrDecisionError::AmbiguousManagedPullRequests { .. })
        ));

        let mut foreign = pr(1, "callisto/version-packages");
        foreign.head_repository = "fork/callisto".to_string();
        assert!(matches!(
            ReleasePrDecisionV2::derive(true, &config(), &snapshot(vec![foreign])),
            Err(ReleasePrDecisionError::ForeignManagedPullRequest { .. })
        ));

        assert!(matches!(
            ReleasePrDecisionV2::derive(
                true,
                &config(),
                &snapshot(vec![pr(1, "callisto/version-packages--bad")])
            ),
            Err(ReleasePrDecisionError::MalformedManagedBranch { .. })
        ));
    }

    #[test]
    fn deserialization_rejects_unknown_fields_and_schema_mismatch() {
        let unknown = r#"{"schemaVersion":1,"repository":"orin-dx/callisto","baseBranch":"main","releaseBranch":"callisto/version-packages","unexpected":true}"#;
        assert!(serde_json::from_str::<ReleasePrConfigV1>(unknown).is_err());
        let version = r#"{"schemaVersion":2,"repository":"orin-dx/callisto","baseBranch":"main","releaseBranch":"callisto/version-packages"}"#;
        assert!(serde_json::from_str::<ReleasePrConfigV1>(version).is_err());
    }

    #[test]
    fn snapshot_v2_rejects_v1_wire_and_unknown_fields() {
        // The v1 shape lacked `headCommit` and had `workflowDeltaFromBase` instead.
        let v1_shaped = r#"{"schemaVersion":2,"repository":"orin-dx/callisto","baseBranch":"main","baseCommit":"0123456789abcdef0123456789abcdef01234567","openPullRequests":[{"number":1,"headRepository":"orin-dx/callisto","headBranch":"callisto/version-packages","workflowDeltaFromBase":false}]}"#;
        assert!(serde_json::from_str::<ReleasePrSnapshotV2>(v1_shaped).is_err());

        let wrong_version = r#"{"schemaVersion":1,"repository":"orin-dx/callisto","baseBranch":"main","baseCommit":"0123456789abcdef0123456789abcdef01234567","openPullRequests":[]}"#;
        assert!(serde_json::from_str::<ReleasePrSnapshotV2>(wrong_version).is_err());
    }

    #[test]
    fn snapshot_recheck_is_order_independent_but_rejects_a_moved_head_or_race() {
        let decision =
            ReleasePrDecisionV2::derive(true, &config(), &snapshot(vec![pr(42, "callisto/version-packages")])).unwrap();
        let same = snapshot(vec![pr(42, "callisto/version-packages")]);
        assert!(decision.verify_snapshot(&same).is_ok());

        let mut moved_head = pr(42, "callisto/version-packages");
        moved_head.head_commit = sha();
        let moved = snapshot(vec![moved_head]);
        assert!(matches!(
            decision.verify_snapshot(&moved),
            Err(ReleasePrDecisionError::SnapshotChanged)
        ));

        let changed = snapshot(vec![pr(43, "callisto/version-packages")]);
        assert!(matches!(
            decision.verify_snapshot(&changed),
            Err(ReleasePrDecisionError::SnapshotChanged)
        ));
    }

    fn added(path: &str, contents: &[u8]) -> StagedChangeV1 {
        StagedChangeV1 {
            path: path.to_string(),
            kind: StagedChangeKindV1::Added,
            new_mode: Some(REGULAR_FILE_MODE),
            contents: Some(contents.to_vec()),
        }
    }

    fn deleted(path: &str) -> StagedChangeV1 {
        StagedChangeV1 {
            path: path.to_string(),
            kind: StagedChangeKindV1::Deleted,
            new_mode: None,
            contents: None,
        }
    }

    #[test]
    fn commit_plan_rejects_empty_change_set() {
        assert!(matches!(
            ReleasePrCommitPlanV1::from_changes(sha(), "msg".to_string(), vec![]),
            Err(ReleasePrDecisionError::EmptyCommitPlan { .. })
        ));
    }

    #[test]
    fn commit_plan_rejects_non_regular_modes() {
        let executable = StagedChangeV1 {
            path: "script.sh".to_string(),
            kind: StagedChangeKindV1::Added,
            new_mode: Some(0o100755),
            contents: Some(b"echo hi".to_vec()),
        };
        assert!(matches!(
            ReleasePrCommitPlanV1::from_changes(sha(), "msg".to_string(), vec![executable]),
            Err(ReleasePrDecisionError::UnsupportedFileMode { mode: 0o100755, .. })
        ));

        let symlink = StagedChangeV1 {
            path: "link".to_string(),
            kind: StagedChangeKindV1::Added,
            new_mode: Some(0o120000),
            contents: Some(b"target".to_vec()),
        };
        assert!(matches!(
            ReleasePrCommitPlanV1::from_changes(sha(), "msg".to_string(), vec![symlink]),
            Err(ReleasePrDecisionError::UnsupportedFileMode { mode: 0o120000, .. })
        ));
    }

    #[test]
    fn commit_plan_rejects_rename_copy_type_change_and_unmerged() {
        for kind in [
            StagedChangeKindV1::Renamed,
            StagedChangeKindV1::Copied,
            StagedChangeKindV1::TypeChanged,
            StagedChangeKindV1::Unmerged,
        ] {
            let change = StagedChangeV1 {
                path: "x".to_string(),
                kind,
                new_mode: Some(REGULAR_FILE_MODE),
                contents: Some(vec![]),
            };
            assert!(matches!(
                ReleasePrCommitPlanV1::from_changes(sha(), "msg".to_string(), vec![change]),
                Err(ReleasePrDecisionError::UnsupportedChangeKind { .. })
            ));
        }
    }

    #[test]
    fn commit_plan_rejects_workflow_and_unsafe_paths() {
        for path in [
            ".github/workflows/ci.yml",
            ".github/workflows",
            "../escape.txt",
            "a/../../escape.txt",
            "/absolute.txt",
            ".git/config",
        ] {
            assert!(
                matches!(
                    ReleasePrCommitPlanV1::from_changes(sha(), "msg".to_string(), vec![added(path, b"x")]),
                    Err(ReleasePrDecisionError::ForbiddenCommitPlanPath { .. })
                ),
                "expected {path} to be forbidden"
            );
        }
    }

    #[test]
    fn commit_plan_rejects_oversized_payload() {
        let huge = vec![0u8; MAX_COMMIT_PLAN_BYTES + 1];
        assert!(matches!(
            ReleasePrCommitPlanV1::from_changes(sha(), "msg".to_string(), vec![added("big.bin", &huge)]),
            Err(ReleasePrDecisionError::CommitPlanTooLarge { .. })
        ));
    }

    #[test]
    fn commit_plan_base64_round_trips_crlf_and_binary_bytes() {
        let crlf = b"line one\r\nline two\r\n".to_vec();
        let binary: Vec<u8> = (0..=255u8).collect();
        let plan = ReleasePrCommitPlanV1::from_changes(
            sha(),
            "msg".to_string(),
            vec![
                added("crlf.txt", &crlf),
                added("binary.bin", &binary),
                deleted("old.md"),
            ],
        )
        .unwrap();
        assert_eq!(plan.additions.len(), 2);
        assert_eq!(
            plan.deletions,
            vec![ReleasePrFileDeletionV1 {
                path: "old.md".to_string()
            }]
        );
        let crlf_entry = plan.additions.iter().find(|a| a.path == "crlf.txt").unwrap();
        assert_eq!(BASE64.decode(&crlf_entry.contents_base64).unwrap(), crlf);
        let binary_entry = plan.additions.iter().find(|a| a.path == "binary.bin").unwrap();
        assert_eq!(BASE64.decode(&binary_entry.contents_base64).unwrap(), binary);
        assert_eq!(plan.total_content_bytes, crlf.len() + binary.len());
    }

    #[test]
    fn commit_plan_serializes_camel_case_and_schema_version() {
        let plan = ReleasePrCommitPlanV1::from_changes(
            sha(),
            "chore(release): version packages".to_string(),
            vec![added("VERSION", b"1.2.3")],
        )
        .unwrap();
        let value = serde_json::to_value(&plan).unwrap();
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["baseCommit"], sha().as_str());
        assert_eq!(value["additions"][0]["path"], "VERSION");
        assert!(value["additions"][0].get("contentsBase64").is_some());
        assert!(value.get("total_content_bytes").is_none());

        let round_tripped: ReleasePrCommitPlanV1 = serde_json::from_value(value).unwrap();
        assert_eq!(round_tripped, plan);
    }
}
