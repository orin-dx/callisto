//! Standalone callisto CLI binary library target (`wrapper` feature for callisto-moon).

#![allow(clippy::result_large_err)]

pub mod cli;
pub mod commands;
pub mod error;
pub mod output;
pub mod render;
pub mod runner;
pub mod tty;
pub mod workspace;

pub use cli::{Cli, Command, GlobalArgs, OutputFormat};
pub use error::CliError;
pub use output::{log_line, write_json};
pub use runner::CliCommandRunner;
