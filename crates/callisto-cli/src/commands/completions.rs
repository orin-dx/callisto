use std::io;
use std::process::ExitCode;

use clap::CommandFactory;
use clap_complete::generate;

use crate::cli::{Cli, CompletionsArgs, GlobalArgs};
use crate::error::CliError;

pub fn handle(args: CompletionsArgs, _global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let mut cmd = Cli::command();
    generate(args.shell, &mut cmd, "callisto", &mut io::stdout());
    Ok(ExitCode::SUCCESS)
}
