//! Standalone callisto CLI binary library target.

#![allow(clippy::result_large_err)]

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
