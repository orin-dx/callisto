use callisto_graph::locate::LocateError;
use callisto_graph::{ConfigError, GraphError};
use callisto_model::CommandError;
use miette::Diagnostic;

#[derive(Debug, thiserror::Error, Diagnostic)]
#[non_exhaustive]
pub enum CliError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Graph(#[from] GraphError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Locate(#[from] LocateError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Command(#[from] CommandError),

    #[error(transparent)]
    #[diagnostic(
        code(callisto::registry_error),
        help("verify registry credentials/authentication and network connectivity, then retry")
    )]
    Registry(#[from] callisto_model::RegistryError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    ChangesetParse(#[from] callisto_format::ParseError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    ChangesetWrite(#[from] callisto_format::WriteError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Manifest(#[from] callisto_model::ManifestError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Vcs(#[from] callisto_vcs::VcsError),

    #[error(transparent)]
    #[diagnostic(code(callisto::pre_json_error))]
    PreJson(#[from] callisto_format::PreJsonError),

    #[error("I/O error{}", match &path {
        Some(p) => format!(" accessing `{}`", p.display()),
        None => String::new(),
    })]
    #[diagnostic(
        code(callisto::io_error),
        help("check that the path exists and that you have permission to access it")
    )]
    Io {
        #[source]
        source: std::io::Error,
        path: Option<std::path::PathBuf>,
    },

    #[error(
        "refusing to prompt interactively: stdin is not a terminal and no non-interactive flags were given"
    )]
    #[diagnostic(
        code(callisto::not_a_tty),
        help("specify package names explicitly via `callisto add --package <name>:<severity>` in CI environments")
    )]
    NotATty,

    #[error("{0}")]
    #[diagnostic(code(callisto::error))]
    Other(String),
}

impl From<std::io::Error> for CliError {
    fn from(source: std::io::Error) -> Self {
        CliError::Io { source, path: None }
    }
}
