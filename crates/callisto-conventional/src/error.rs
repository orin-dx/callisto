use std::path::PathBuf;

use callisto_model::CommandError;

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConventionalError {
    #[error(transparent)]
    Command(#[from] CommandError),

    #[error("`git log` failed in `{cwd}`: {stderr}")]
    GitLogFailed { cwd: PathBuf, stderr: String },

    #[error("could not parse `git log` output in `{cwd}` into commit records: {message}")]
    MalformedGitLogOutput { cwd: PathBuf, message: String },

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
