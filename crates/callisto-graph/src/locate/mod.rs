use std::path::PathBuf;

use callisto_model::{DeclaredEdge, ProjectRoot};

pub mod git;
pub mod ignore_walk;
pub mod root;

pub use git::probe_git;
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
    #[diagnostic(code(E030), help("Ensure callisto.toml exists in workspace root."))]
    WorkspaceRootNotFound { start: PathBuf },

    #[error("failed to walk filesystem under `{path}`: {message}")]
    #[diagnostic(code(E031))]
    Walk { path: PathBuf, message: String },

    #[error("project path `{path}` is outside the workspace root `{root}`")]
    #[diagnostic(code(E032))]
    OutsideWorkspaceRoot { path: PathBuf, root: PathBuf },

    #[error("moon CLI is unavailable or exited non-zero")]
    MoonUnavailable,

    #[error("Failed to parse moon project-graph output: {message}")]
    MoonOutputParse { message: String },

    #[error("Incompatible moon version found: {found}, required: {required}")]
    IncompatibleMoonVersion { found: String, required: String },

    #[error(transparent)]
    Graph(#[from] Box<crate::error::GraphError>),
}
