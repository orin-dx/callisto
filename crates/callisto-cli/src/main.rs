#![allow(clippy::result_large_err)]

use std::process::ExitCode;

use clap::Parser;

use callisto_cli::cli::{Cli, Command};
use callisto_cli::commands::*;

// Exit codes: 0 success, 1 failure, 2 usage error: from clap, or a handler error `CliError::is_usage_error` classifies (docs/stability.md). A handler's `Ok(code)` is returned as is.

fn main() -> ExitCode {
    let cli = Cli::parse();
    let command_name = cli.command.report_name();

    let res = match cli.command {
        Command::Add(args) => add::handle(args, &cli.global),
        Command::Status(args) => status::handle(args, &cli.global),
        Command::Matrix(args) => matrix::handle(args, &cli.global),
        Command::Version(args) => version::handle(args, &cli.global),
        Command::Pre(args) => pre::handle(args, &cli.global),
        Command::Snapshot(args) => snapshot::handle(args, &cli.global),
        Command::Init(args) => init::handle(args, &cli.global),
        Command::ComposePrBody(args) => compose_pr_body::handle(args, &cli.global),
        Command::Release(args) => release::handle(args, &cli.global),
        Command::ReleasePr(args) => release_pr::handle(args, &cli.global),
        Command::Completions(args) => completions::handle(args, &cli.global),
        Command::Schema(args) => schema::handle(args, &cli.global),
        Command::Help(args) => help::handle(args, &cli.global),
    };

    match res {
        Ok(code) => code,
        Err(err) => {
            let exit = if err.is_usage_error() {
                ExitCode::from(2)
            } else {
                ExitCode::FAILURE
            };
            if cli.global.format == callisto_cli::cli::OutputFormat::Json {
                let report = callisto_cli::error::format_error_json(command_name, cli.global.dry_run, &err);
                let _res = callisto_cli::output::write_error_json(&mut std::io::stderr(), &report);
            } else {
                let indeterminate = matches!(
                    miette::Diagnostic::code(&err).map(|code| code.to_string()).as_deref(),
                    Some("E157" | "E176")
                );
                eprintln!("{:?}", miette::Report::new(err));
                if let Some(summary) = indeterminate.then(callisto_cli::runner::last_failed_command).flatten() {
                    eprintln!("Most recent failed command (redacted):\n{summary}");
                }
            }
            exit
        }
    }
}
