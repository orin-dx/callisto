use callisto_graph::locate::LocateError;
use callisto_graph::{ConfigError, GraphError};
use callisto_model::CommandError;
use miette::Diagnostic;

#[derive(Debug, thiserror::Error, Diagnostic)]
#[non_exhaustive]
pub enum CliError {
    #[error(transparent)]
    #[diagnostic(code(callisto::graph_error))]
    Graph(#[from] GraphError),

    #[error(transparent)]
    #[diagnostic(code(callisto::locate_error))]
    Locate(#[from] LocateError),

    #[error(transparent)]
    #[diagnostic(code(callisto::config_error))]
    Config(#[from] ConfigError),

    #[error(transparent)]
    #[diagnostic(code(callisto::command_error))]
    Command(#[from] CommandError),

    #[error(transparent)]
    #[diagnostic(code(callisto::changeset_parse_error))]
    ChangesetParse(#[from] callisto_format::ParseError),

    #[error(transparent)]
    #[diagnostic(code(callisto::changeset_write_error))]
    ChangesetWrite(#[from] callisto_format::WriteError),

    #[error(transparent)]
    #[diagnostic(code(callisto::pre_json_error))]
    PreJson(#[from] callisto_format::PreJsonError),

    #[error(transparent)]
    #[diagnostic(code(callisto::io_error))]
    Io(#[from] std::io::Error),

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
