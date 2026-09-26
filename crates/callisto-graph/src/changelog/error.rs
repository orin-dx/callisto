use std::path::PathBuf;

#[derive(Clone, Debug, thiserror::Error, miette::Diagnostic, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChangelogError {
    #[error("changelog rendering requires at least one entry, but received empty input")]
    #[diagnostic(code(E060))]
    EmptyInput,

    #[error("cannot render changelog entry with Severity::None")]
    #[diagnostic(code(E061))]
    SeverityNoneEntry,

    #[error("failed to read changelog at `{path}`: {message}")]
    #[diagnostic(code(E062))]
    ReadFailed { path: PathBuf, message: String },

    #[error("failed to write changelog at `{path}`: {message}")]
    #[diagnostic(code(E063))]
    WriteFailed { path: PathBuf, message: String },
}
