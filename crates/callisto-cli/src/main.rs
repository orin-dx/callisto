#![allow(clippy::result_large_err)]

use std::process::ExitCode;

use clap::Parser;

use callisto_cli::cli::{Cli, Command};
use callisto_cli::commands::*;

fn main() -> ExitCode {
    let cli = Cli::parse();

    let res = match cli.command {
        Command::Add(args) => add::handle(args, &cli.global),
        Command::Status(args) => status::handle(args, &cli.global),
        Command::Version(args) => version::handle(args, &cli.global),
        Command::Pre(args) => pre::handle(args, &cli.global),
        Command::Validate(args) => validate::handle(args, &cli.global),
        Command::Snapshot(args) => snapshot::handle(args, &cli.global),
        Command::Init(args) => init::handle(args, &cli.global),
        Command::PlanPublish(args) => plan_publish::handle(args, &cli.global),
        Command::ComposePrBody(args) => compose_pr_body::handle(args, &cli.global),
        Command::Tag(args) => tag::handle(args, &cli.global),
        Command::Completions(args) => completions::handle(args, &cli.global),
        Command::Schema(args) => schema::handle(args, &cli.global),
    };

    match res {
        Ok(code) => code,
        Err(err) => {
            if cli.global.format == callisto_cli::cli::OutputFormat::Json {
                let report = serde_json::json!({
                    "schemaVersion": callisto_model::SCHEMA_VERSION,
                    "error": {
                        "message": err.to_string(),
                    }
                });
                let _res = callisto_cli::output::write_json(&mut std::io::stderr(), &report);
            } else {
                eprintln!("{:?}", miette::Report::new(err));
            }
            ExitCode::FAILURE
        }
    }
}
