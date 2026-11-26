//! Standalone callisto CLI binary library target.

#![allow(clippy::result_large_err)]
// Output goes through `output`: stdout through its sink, advisory stderr through `advise`.
#![deny(clippy::print_stdout, clippy::print_stderr)]

pub mod cli;
pub mod color;
pub mod commands;
pub mod error;
pub mod io;
pub mod output;
pub mod render;
pub mod runner;
pub mod tty;
pub mod workspace;

pub use cli::Cli;
