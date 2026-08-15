use std::path::PathBuf;

use callisto_model::{DeclaredEdge, ProjectRoot};

pub mod ignore_walk;
mod membership;
pub mod root;

pub use ignore_walk::IgnoreWalkLocator;
pub use root::find_workspace_root;

pub trait ProjectLocator: Send + Sync {
    fn projects(&self) -> Result<Vec<ProjectRoot>, LocateError>;
    fn declared_edges(&self) -> Option<Vec<DeclaredEdge>> {
        None
    }
}

#[derive(Clone, Debug, thiserror::Error, miette::Diagnostic, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocateError {
    #[error("workspace root not found starting from `{start}`")]
    #[diagnostic(
        code(E030),
        help(
            "No workspace root found. Ensure the directory tree contains a workspace manifest: \
             Cargo.toml with [workspace], package.json with a workspaces field, \
             pnpm-workspace.yaml, or a .moon directory."
        )
    )]
    WorkspaceRootNotFound { start: PathBuf },

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

    #[error("moon CLI is unavailable or exited non-zero")]
    MoonUnavailable,

    #[error("Failed to parse moon project-graph output: {message}")]
    MoonOutputParse { message: String },

    #[error("Incompatible moon version found: {found}, required: {required}")]
    IncompatibleMoonVersion { found: String, required: String },

    #[error(transparent)]
    Graph(#[from] Box<crate::error::GraphError>),
}

impl From<callisto_vcs::VcsError> for LocateError {
    fn from(e: callisto_vcs::VcsError) -> Self {
        LocateError::Vcs(Box::new(e))
    }
}
