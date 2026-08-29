#![allow(clippy::result_large_err)]

use std::process::ExitCode;

use clap::Parser;

use callisto_cli::cli::{Cli, Command};
use callisto_cli::commands::*;

// Exit code contract for every subcommand
// =========================================
//
// Default rule: every `Err(CliError)` arm below maps to exit code 1 (FAILURE).
// `Ok(code)` is returned verbatim, so each handler owns its own success codes.
//
//   add            0 on success (changeset written or dry-run preview)
//                  1 on any error (NotATty, invalid package spec, I/O, etc.)
//
//   status         0 when no errors and --check is not set
//                  1 when any diagnostic is Error-severity (or Warning under --strict),
//                    OR when --check is set and at least one package has pending changesets
//                  2 (ExitCode::from(2)) when --check is set, there are no diagnostic
//                    errors, and no changesets are pending
//
//   matrix         0 on success (report printed, including an empty report or
//                  one carrying UnrecognisedPlatformTriple warnings)
//                  1 on any error (unknown --package, malformed/wrong-type
//                  manifest, conflicting platform-target sources)
//
//   version        0 on success (versions bumped or dry-run preview)
//                  1 on any error (graph error, strict violation, etc.)
//
//   pre            0 on success
//                  1 on any error
//
//   validate       0 when the workspace passes all checks
//                  1 on any validation error or strict violation
//
//   snapshot       0 on success (snapshot versions applied or dry-run preview)
//                  1 on any error (graph error, strict crosscheck failure, etc.)
//
//   init           0 on success (configuration scaffolded or dry-run preview)
//                  1 on any error
//
//   plan-publish   0 on success
//                  1 on any error
//
//   publish        0 on success
//                  1 on any error
//
//   compose-pr-body  0 on success
//                    1 on any error
//
//   tag            0 on success (tags created or dry-run preview)
//                  1 on any error (graph error, strict crosscheck failure, etc.)
//
//   completions    0 on success
//                  1 on any error
//
//   schema         0 on success
//                  1 on any error

fn main() -> ExitCode {
    let cli = Cli::parse();

    let res = match cli.command {
        Command::Add(args) => add::handle(args, &cli.global),
        Command::Status(args) => status::handle(args, &cli.global),
        Command::Matrix(args) => matrix::handle(args, &cli.global),
        Command::Version(args) => version::handle(args, &cli.global),
        Command::Pre(args) => pre::handle(args, &cli.global),
        Command::Validate(args) => validate::handle(args, &cli.global),
        Command::Snapshot(args) => snapshot::handle(args, &cli.global),
        Command::Init(args) => init::handle(args, &cli.global),
        Command::PlanPublish(args) => plan_publish::handle(args, &cli.global),
        Command::Publish(args) => publish::handle(args, &cli.global),
        Command::ComposePrBody(args) => compose_pr_body::handle(args, &cli.global),
        Command::Tag(args) => tag::handle(args, &cli.global),
        Command::FilterPlan(args) => filter_plan::handle(args, &cli.global),
        Command::Completions(args) => completions::handle(args, &cli.global),
        Command::Schema(args) => schema::handle(args, &cli.global),
    };

    match res {
        Ok(code) => code,
        Err(err) => {
            if cli.global.format == callisto_cli::cli::OutputFormat::Json {
                let report = callisto_cli::error::format_error_json(&err);
                let _res = callisto_cli::output::write_json(&mut std::io::stderr(), &report);
            } else {
                eprintln!("{:?}", miette::Report::new(err));
            }
            ExitCode::FAILURE
        }
    }
}
