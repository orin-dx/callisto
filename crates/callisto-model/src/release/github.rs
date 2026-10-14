use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::ArtifactSlotError;
use crate::identity::string_json_schema;
use crate::CommitSha;

/// An exact `owner/repo` GitHub repository identity.
///
/// Replaces three previously independent, inconsistent ad hoc validations of
/// the same concept (a bare `split_once('/')` check with no character-class
/// restriction, a stricter ASCII-alphanumeric-plus-`-_.` check, and an
/// entirely unvalidated `format!("{owner}/{repository}")`); every caller now
/// parses through this single charset rule instead.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GitHubRepository {
    owner: String,
    repo: String,
}

string_json_schema!(GitHubRepository);

impl GitHubRepository {
    /// Parses an exact `owner/repo` GitHub repository identity.
    ///
    /// A trailing `.git` on the repository is dropped and both parts are
    /// lowercased. Both `owner` and `repo` must be non-empty, ASCII alphanumeric plus
    /// `-`, `_`, and `.`, and must not start or end with `-`. This is
    /// deliberately one charset rule applied uniformly to both parts,
    /// matching GitHub's own allowed repository-name charset closely enough
    /// to reject the unsafe inputs that matter (for example, embedded
    /// whitespace) without re-implementing GitHub's full, occasionally
    /// stricter, username rules.
    pub fn parse(s: &str) -> Result<Self, GitHubRepositoryParseError> {
        let Some((owner, repo)) = s.split_once('/') else {
            return Err(GitHubRepositoryParseError::MissingSeparator { raw: s.to_string() });
        };
        if repo.contains('/') {
            return Err(GitHubRepositoryParseError::TooManyParts { raw: s.to_string() });
        }
        let repo = repo.strip_suffix(".git").unwrap_or(repo);
        if matches!(owner, "." | "..") || matches!(repo, "." | "..") {
            return Err(GitHubRepositoryParseError::DotComponent { raw: s.to_string() });
        }
        if !is_valid_github_repository_part(owner) {
            return Err(GitHubRepositoryParseError::InvalidOwner { raw: s.to_string() });
        }
        if !is_valid_github_repository_part(repo) {
            return Err(GitHubRepositoryParseError::InvalidRepo { raw: s.to_string() });
        }
        // GitHub names are case-insensitive, so the lowercase form is the identity.
        Ok(Self {
            owner: owner.to_ascii_lowercase(),
            repo: repo.to_ascii_lowercase(),
        })
    }

    /// Returns the owner component.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Returns the repository-name component.
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// Renders the canonical `owner/repo` slug, for example for `gh --repo`.
    pub fn as_slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

impl fmt::Display for GitHubRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.repo)
    }
}

impl FromStr for GitHubRepository {
    type Err = GitHubRepositoryParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl PartialOrd for GitHubRepository {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for GitHubRepository {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.owner.as_str(), self.repo.as_str()).cmp(&(other.owner.as_str(), other.repo.as_str()))
    }
}

impl Serialize for GitHubRepository {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_slug())
    }
}

impl<'de> Deserialize<'de> for GitHubRepository {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

fn is_valid_github_repository_part(part: &str) -> bool {
    !part.is_empty()
        && !part.starts_with('-')
        && !part.ends_with('-')
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// Errors produced while parsing a [`GitHubRepository`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum GitHubRepositoryParseError {
    #[error("GitHub repository `{raw}` must be in owner/repo form")]
    MissingSeparator { raw: String },
    #[error("GitHub repository `{raw}` has more than one `/`")]
    TooManyParts { raw: String },
    #[error("GitHub repository `{raw}` has an invalid owner")]
    InvalidOwner { raw: String },
    #[error("GitHub repository `{raw}` has an invalid repository name")]
    InvalidRepo { raw: String },
    #[error("GitHub repository `{raw}` has a `.` or `..` component")]
    DotComponent { raw: String },
}

/// The one workflow whose runs may attest release artifacts; CI asserts the file exists here.
pub const RELEASE_COORDINATOR_WORKFLOW_PATH: &str = ".github/workflows/callisto-release.yml";

/// Immutable GitHub provenance policy declared by a binary release slot.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitHubAttestationPolicyV1 {
    pub repository: GitHubRepository,
    pub workflow_path: String,
    pub workflow_commit: CommitSha,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GitHubAttestationPolicyV1Wire {
    repository: GitHubRepository,
    workflow_path: String,
    workflow_commit: CommitSha,
}

impl GitHubAttestationPolicyV1 {
    pub fn new(
        repository: GitHubRepository,
        workflow_path: impl Into<String>,
        workflow_commit: CommitSha,
    ) -> Result<Self, ArtifactSlotError> {
        let workflow_path = workflow_path.into();
        if !is_safe_workflow_path(&workflow_path) {
            return Err(ArtifactSlotError::UnsafeSlotComponent);
        }
        Ok(Self {
            repository,
            workflow_path,
            workflow_commit,
        })
    }
}

impl<'de> Deserialize<'de> for GitHubAttestationPolicyV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = GitHubAttestationPolicyV1Wire::deserialize(deserializer)?;
        Self::new(wire.repository, wire.workflow_path, wire.workflow_commit).map_err(serde::de::Error::custom)
    }
}

fn is_safe_workflow_path(value: &str) -> bool {
    value.starts_with(".github/workflows/")
        && value.ends_with(".yml")
        && !value.contains("..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
}
