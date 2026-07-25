use std::path::PathBuf;

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChangelogError {
    #[error("changelog rendering requires at least one entry, but received empty input")]
    EmptyInput,

    #[error("cannot render changelog entry with Severity::None")]
    SeverityNoneEntry,

    #[error("failed to read changelog at `{path}`: {message}")]
    ReadFailed { path: PathBuf, message: String },

    #[error("failed to write changelog at `{path}`: {message}")]
    WriteFailed { path: PathBuf, message: String },
}
