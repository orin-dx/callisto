//! The Layer 1 commit-history contract.
//!
//! Severity inference needs exactly one thing from a version-control system:
//! the list of commits reachable from `HEAD` down to some bound, scoped to a
//! set of paths. That need is expressed here, in permissive Layer 1, as
//! [`CommitWalker`] over [`CommitRecord`] values -- so consumers such as
//! `callisto-conventional` depend on the *shape* of a commit walk rather than
//! on any particular VCS implementation (native `gix`, a shelled-out `git`,
//! or a test double).

use std::path::PathBuf;

use crate::{CommandError, CommitSha};

/// A single commit as far as history analysis is concerned: its identity plus
/// its message, pre-split at the first blank line the way `git log` splits
/// `%s` from `%b`.
///
/// `summary` is the first line; `body` is everything after the blank line that
/// follows it, or `None` when the message has no body. Callers that need the
/// original raw message rejoin them with a blank line between.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitRecord {
    pub sha: CommitSha,
    pub summary: String,
    pub body: Option<String>,
}

/// Why a [`CommitWalker::commits_since`] call could not produce a history.
///
/// Deliberately narrower than any backend's own error type: it keeps only the
/// distinctions a consumer can act on -- the underlying command failed, the
/// requested bound does not exist, or the backend failed for some other
/// reason it can only describe in prose. Backends map their richer errors
/// into these variants at the boundary.
#[derive(Clone, Debug, thiserror::Error, miette::Diagnostic, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommitWalkError {
    /// The walk was served by shelling out, and the subprocess itself failed
    /// (e.g. no `git` binary on `PATH`).
    #[error(transparent)]
    Command(#[from] CommandError),

    /// An explicitly requested `since_ref` does not resolve to a commit.
    ///
    /// This is always an error rather than a silent fall-through to an
    /// unbounded walk: ignoring the caller's bound would re-surface
    /// already-released commits into severity and changelog inference.
    #[error("reference `{ref_name}` was not found")]
    #[diagnostic(
        code(E054),
        help("Check if the reference or tag exists in local or remote Git refs.")
    )]
    RefNotFound { ref_name: String },

    /// The backend failed for a reason with no Layer 1 equivalent -- a
    /// repository that could not be opened, a corrupt object database, an
    /// unparsable log stream. `message` carries the backend's own rendering.
    #[error("commit walk failed: {message}")]
    #[diagnostic(code(E026))]
    Backend { message: String },
}

/// Reads commit history. The single VCS capability that severity inference
/// requires, and therefore the only one this trait exposes.
pub trait CommitWalker {
    /// Lists commits reachable from `HEAD`, down to (exclusive) `since_ref`
    /// when given, filtered to those touching at least one of `pathspecs`.
    ///
    /// An empty `pathspecs` slice disables path filtering and returns every
    /// commit in the walk. Merge commits are excluded, matching
    /// `git log --no-merges`. Results are ordered newest-first.
    ///
    /// `since_ref: None` requests the full history and always succeeds;
    /// a `since_ref` that is given but does not resolve is
    /// [`CommitWalkError::RefNotFound`], never a silent unbounded walk.
    fn commits_since(
        &self,
        since_ref: Option<&str>,
        pathspecs: &[PathBuf],
    ) -> Result<Vec<CommitRecord>, CommitWalkError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec: the trait is object-safe. Consumers take `&dyn CommitWalker`
    /// so that a single non-generic function body serves every backend.
    #[test]
    fn test_commit_walker_is_object_safe() {
        struct Empty;
        impl CommitWalker for Empty {
            fn commits_since(
                &self,
                _since_ref: Option<&str>,
                _pathspecs: &[PathBuf],
            ) -> Result<Vec<CommitRecord>, CommitWalkError> {
                Ok(Vec::new())
            }
        }

        let walker: &dyn CommitWalker = &Empty;
        assert!(walker.commits_since(None, &[]).unwrap().is_empty());
    }

    /// Spec: a `CommandError` converts into `CommitWalkError` transparently,
    /// so a shelled-out backend can propagate spawn failures with `?`.
    #[test]
    fn test_command_error_converts_transparently() {
        let err: CommitWalkError = CommandError::NotFound {
            program: "git".to_string(),
        }
        .into();

        assert!(matches!(err, CommitWalkError::Command(_)));
        assert_eq!(
            err.to_string(),
            "`git` was not found; callisto requires it to be available"
        );
    }
}
