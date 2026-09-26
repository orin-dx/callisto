use std::path::PathBuf;

use callisto_model::{CommandError, CommitWalkError};

#[derive(Clone, Debug, thiserror::Error, miette::Diagnostic, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConventionalError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Command(#[from] CommandError),

    /// Any failure reported by the [`callisto_model::CommitWalker`] backing
    /// severity inference -- `git log` itself failing, an unparsable log
    /// stream, or an explicitly-requested `since` ref that doesn't resolve
    /// to a commit.
    ///
    /// Named for the Layer 1 contract, not for whichever VCS engine happens
    /// to satisfy it: this crate parses conventional commits and has no
    /// opinion on where the history came from.
    #[error(transparent)]
    #[diagnostic(transparent)]
    CommitWalk(#[from] CommitWalkError),

    #[error("pre-cursor ref `{ref_name}` in `{cwd}` could not be resolved: {stderr}")]
    #[diagnostic(
        code(E257),
        help("Check that the pre-cursor ref exists in local or remote Git refs.")
    )]
    MalformedPreCursorRef {
        cwd: PathBuf,
        ref_name: String,
        stderr: String,
    },

    #[error("failed to advance pre-cursor ref `{ref_name}` in `{cwd}` to `{sha}`: {stderr}")]
    #[diagnostic(
        code(E258),
        help("Check repository write permissions and that `{sha}` is a valid commit.")
    )]
    PreCursorAdvanceFailed {
        cwd: PathBuf,
        ref_name: String,
        sha: String,
        stderr: String,
    },
}
