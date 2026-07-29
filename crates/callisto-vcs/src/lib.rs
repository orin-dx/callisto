use std::path::{Path, PathBuf};

use callisto_model::{CommitSha, TagName};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum VcsError {
    #[error("failed to discover Git repository at `{path}`: {message}")]
    RepoNotFound { path: PathBuf, message: String },

    #[error("git error: {0}")]
    Git(String),

    #[error("reference `{ref_name}` was not found")]
    RefNotFound { ref_name: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitCommit {
    pub sha: CommitSha,
    pub summary: String,
    pub body: Option<String>,
}

/// Trait for Git VCS operations.
pub trait GitVcsProvider {
    fn head_sha(&self) -> Result<CommitSha, VcsError>;
    fn list_tags(&self, glob_pattern: Option<&str>) -> Result<Vec<TagName>, VcsError>;
}

pub struct GitRepository {
    #[cfg(not(target_arch = "wasm32"))]
    repo: gix::Repository,
}

impl GitRepository {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn discover(path: impl AsRef<Path>) -> Result<Self, VcsError> {
        let p = path.as_ref();
        let repo = gix::discover(p).map_err(|e| VcsError::RepoNotFound {
            path: p.to_path_buf(),
            message: e.to_string(),
        })?;
        Ok(Self { repo })
    }

    #[cfg(target_arch = "wasm32")]
    pub fn discover(path: impl AsRef<Path>) -> Result<Self, VcsError> {
        Err(VcsError::RepoNotFound {
            path: path.as_ref().to_path_buf(),
            message: "gix native git operations disabled on WASM target".to_string(),
        })
    }

    pub fn head_sha(&self) -> Result<CommitSha, VcsError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let head = self
                .repo
                .head_commit()
                .map_err(|e| VcsError::Git(format!("Failed to get HEAD commit: {e}")))?;
            CommitSha::parse(&head.id.to_hex().to_string())
                .map_err(|e| VcsError::Git(format!("Invalid HEAD SHA: {e}")))
        }
        #[cfg(target_arch = "wasm32")]
        {
            Err(VcsError::Git("WASM unsupported".to_string()))
        }
    }

    pub fn list_tags(&self, glob_pattern: Option<&str>) -> Result<Vec<TagName>, VcsError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let platform = self
                .repo
                .references()
                .map_err(|e| VcsError::Git(format!("Failed to read references: {e}")))?;

            let mut tags = Vec::new();
            let tag_refs = platform
                .tags()
                .map_err(|e| VcsError::Git(format!("Failed to list tag refs: {e}")))?;

            let matcher = glob_pattern.and_then(|p| globset::Glob::new(p).ok());

            for r in tag_refs.flatten() {
                let name = r.name().shorten().to_string();
                if let Some(ref m) = matcher {
                    if !m.compile_matcher().is_match(&name) {
                        continue;
                    }
                }
                tags.push(TagName(name));
            }

            Ok(tags)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _unused = glob_pattern;
            Ok(Vec::new())
        }
    }

    pub fn commits_since(&self, from_ref: Option<&str>) -> Result<Vec<GitCommit>, VcsError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let head = self
                .repo
                .head_commit()
                .map_err(|e| VcsError::Git(format!("Failed to get HEAD commit: {e}")))?;

            let revwalk = self
                .repo
                .rev_walk(vec![head.id])
                .all()
                .map_err(|e| VcsError::Git(format!("Failed to create revwalk: {e}")))?;

            let stop_sha = if let Some(r) = from_ref {
                if let Ok(spec) = self.repo.rev_parse_single(r) {
                    if let Ok(object) = spec.object() {
                        if let Ok(commit) = object.peel_to_kind(gix::object::Kind::Commit) {
                            Some(commit.id.to_hex().to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let mut commits = Vec::new();
            for info in revwalk {
                let info = info.map_err(|e| VcsError::Git(e.to_string()))?;
                let hex = info.id.to_hex().to_string();

                if let Some(ref target) = stop_sha {
                    if &hex == target {
                        break;
                    }
                }

                let commit_obj = info
                    .object()
                    .map_err(|e| VcsError::Git(format!("Failed to load commit object: {e}")))?;

                let sha = CommitSha::parse(&hex)
                    .map_err(|e| VcsError::Git(format!("Invalid commit SHA: {e}")))?;

                let message = commit_obj
                    .message()
                    .map_err(|e| VcsError::Git(e.to_string()))?;
                let summary = message.title.to_string();
                let body = message.body.map(|b| b.to_string());

                commits.push(GitCommit { sha, summary, body });
            }

            Ok(commits)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _unused = from_ref;
            Ok(Vec::new())
        }
    }
}

impl GitVcsProvider for GitRepository {
    fn head_sha(&self) -> Result<CommitSha, VcsError> {
        self.head_sha()
    }

    fn list_tags(&self, glob_pattern: Option<&str>) -> Result<Vec<TagName>, VcsError> {
        self.list_tags(glob_pattern)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovers_repo() {
        let repo = GitRepository::discover(".");
        assert!(repo.is_ok());

        let r = repo.unwrap();
        let head = r.head_sha();
        assert!(head.is_ok());
    }
}
