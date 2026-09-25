use std::path::PathBuf;

use callisto_model::{CommandError, CommitRecord, CommitWalkError, CommitWalker};
use thiserror::Error;

pub mod access;

pub use access::GitAccess;

#[derive(Clone, Debug, Error, miette::Diagnostic, PartialEq, Eq)]
#[non_exhaustive]
pub enum VcsError {
    #[error("failed to discover Git repository at `{path}`: {message}")]
    #[diagnostic(code(E050), help("Ensure target directory is inside a valid Git repository."))]
    RepoNotFound { path: PathBuf, message: String },

    #[error("git error: {0}")]
    #[diagnostic(code(E051))]
    Git(String),

    #[error("reference `{ref_name}` was not found")]
    #[diagnostic(code(E052), help("Check if reference or tag exists in local or remote Git refs."))]
    RefNotFound { ref_name: String },

    #[error("tag glob pattern `{pattern}` is not a valid glob: {message}")]
    #[diagnostic(
        code(E053),
        help("Fix the glob syntax (e.g. balance `{{`/`}}` and `[`/`]`) or use a literal tag name.")
    )]
    InvalidGlob { pattern: String, message: String },

    /// The `git` binary itself could not be run. `transparent` so callers
    /// can match the underlying [`CommandError`] through it.
    #[error(transparent)]
    Command(#[from] CommandError),
}

/// Narrows a [`VcsError`] to the Layer 1 [`CommitWalkError`] vocabulary at
/// the [`CommitWalker`] boundary.
///
/// [`CommitWalkError::Command`] and [`CommitWalkError::RefNotFound`] survive
/// as themselves -- they are the two distinctions consumers branch on. Every
/// other variant has no Layer 1 equivalent, so it collapses into [`CommitWalkError::Backend`] carrying this error's
/// own `Display` rendering; nothing is lost from the message a user sees.
impl From<VcsError> for CommitWalkError {
    fn from(err: VcsError) -> Self {
        match err {
            VcsError::Command(inner) => CommitWalkError::Command(inner),
            VcsError::RefNotFound { ref_name } => CommitWalkError::RefNotFound { ref_name },
            other => CommitWalkError::Backend {
                message: other.to_string(),
            },
        }
    }
}

/// A commit as produced by a history walk.
///
/// Aliases [`callisto_model::CommitRecord`] rather than redeclaring it, so a
/// walk's output crosses the [`CommitWalker`] seam without a conversion and
/// there is exactly one definition of a commit's shape in the workspace.
pub type GitCommit = CommitRecord;

/// Compiles `pattern` into a [`globset::GlobMatcher`], surfacing a malformed
/// pattern as [`VcsError::InvalidGlob`] rather than letting it silently
/// disable filtering -- which would match every tag, a real correctness
/// risk for release tagging (a malformed tag template could make "last tag"
/// resolution pick an unrelated package's tag). Shared by
/// [`GitAccess::list_tags`] and `callisto-graph`'s `tags::matching_tags`, so
/// both filter tag names with identical semantics.
pub fn compile_tag_glob(pattern: &str) -> Result<globset::GlobMatcher, VcsError> {
    globset::Glob::new(pattern)
        .map(|g| g.compile_matcher())
        .map_err(|e| VcsError::InvalidGlob {
            pattern: pattern.to_string(),
            message: e.to_string(),
        })
}

/// Whether [`GitAccess::create_tag`] lets the ambient Git signing
/// configuration (`tag.gpgSign`/`commit.gpgsign`) apply, or forces an
/// unsigned tag regardless of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagSignPolicy {
    /// Let the repository's own Git config decide -- the right default for
    /// a human-run tag.
    RespectRepoConfig,
    /// Always pass `--no-sign`, regardless of `tag.gpgSign`/`commit.gpgsign`.
    /// Some CI contexts set those globally (for commit signing) with no
    /// tag-signing key available; the durable release executor uses this to
    /// avoid failing there.
    ForceUnsigned,
}

impl CommitWalker for GitAccess<'_> {
    fn commits_since(
        &self,
        since_ref: Option<&str>,
        pathspecs: &[PathBuf],
    ) -> Result<Vec<CommitRecord>, CommitWalkError> {
        GitAccess::commits_since(self, since_ref, pathspecs).map_err(Into::into)
    }
}
