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
}
