use std::path::PathBuf;

use callisto_model::CommandError;

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConventionalError {
    #[error(transparent)]
    Command(#[from] CommandError),

    /// Any failure sourced from `callisto_vcs::GitDataSource` (native gix
    /// or the `CommandRunner`-shelled fallback) while fetching commits --
    /// e.g. `git log` itself failing, malformed `git log` output, or an
    /// explicitly-requested `since` ref that doesn't resolve to a commit
    /// (see `VcsError::RefNotFound`/`VcsError::Git`). This subsumes what
    /// used to be this crate's own `GitLogFailed`/`MalformedGitLogOutput`
    /// variants, now that `fetch_commits` delegates the shell-out entirely
    /// to `callisto_vcs::ShellGit`.
    #[error(transparent)]
    Vcs(#[from] callisto_vcs::VcsError),

    #[error("pre-cursor ref `{ref_name}` in `{cwd}` could not be resolved: {stderr}")]
    MalformedPreCursorRef {
        cwd: PathBuf,
        ref_name: String,
        stderr: String,
    },

    #[error("failed to advance pre-cursor ref `{ref_name}` in `{cwd}` to `{sha}`: {stderr}")]
    PreCursorAdvanceFailed {
        cwd: PathBuf,
        ref_name: String,
        sha: String,
        stderr: String,
    },
}
