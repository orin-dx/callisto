use std::path::PathBuf;

use callisto_model::ProjectRoot;

pub mod ignore_walk;
mod membership;
pub mod root;

pub use ignore_walk::IgnoreWalkLocator;
pub use root::find_workspace_root;

pub trait ProjectLocator: Send + Sync {
    fn projects(&self) -> Result<Vec<ProjectRoot>, LocateError>;
    /// `projects()`, plus npm platform packages (`os`+`cpu`) outside the npm
    /// workspace's membership: candidates for platform-package attachment only.
    fn projects_and_platform_candidates(&self) -> Result<(Vec<ProjectRoot>, Vec<ProjectRoot>), LocateError> {
        Ok((self.projects()?, Vec::new()))
    }
}

#[derive(Clone, Debug, thiserror::Error, miette::Diagnostic, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocateError {
    #[error("workspace root not found between `{}` and the Git repository root `{}`", .start.display(), .toplevel.display())]
    #[diagnostic(
        code(E030),
        help(
            "Callisto never searches above the Git repository root. Add a workspace manifest \
             (Cargo.toml with [workspace], package.json with a workspaces field, \
             pnpm-workspace.yaml, or a .moon directory) or a package manifest \
             (Cargo.toml with [package], package.json, or pyproject.toml) inside the repository."
        )
    )]
    WorkspaceRootNotFound { start: PathBuf, toplevel: PathBuf },

    #[error("`{}` is not inside a Git repository", .start.display())]
    #[diagnostic(
        code(E058),
        help("Callisto needs a Git repository: run `git init` in the workspace root.")
    )]
    NotAGitRepository { start: PathBuf },

    #[error("failed to walk filesystem under `{path}`: {message}")]
    #[diagnostic(code(E031))]
    Walk { path: PathBuf, message: String },

    #[error("project path `{path}` is outside the workspace root `{root}`")]
    #[diagnostic(code(E032))]
    OutsideWorkspaceRoot { path: PathBuf, root: PathBuf },

    /// A VCS operation failed during workspace location. This variant exists
    /// so that callers who need to distinguish filesystem-structure errors
    /// (WorkspaceRootNotFound, Walk) from VCS errors (e.g., a git repository
    /// that could not be opened or queried during locate) can do so.
    #[error("VCS error during workspace location: {0}")]
    #[diagnostic(code(E033))]
    Vcs(Box<callisto_vcs::VcsError>),

    #[error(transparent)]
    Graph(#[from] Box<crate::error::GraphError>),
}

impl From<callisto_vcs::VcsError> for LocateError {
    fn from(e: callisto_vcs::VcsError) -> Self {
        LocateError::Vcs(Box::new(e))
    }
}
