//! Read-only interface for typed managed release-pull-request decisions.

use std::process::ExitCode;

use callisto_graph::commands::{status, StatusOptions};
use callisto_model::{ReleasePrActionV1, ReleasePrConfigV1, ReleasePrDecisionV1, ReleasePrSnapshotV1};

use crate::cli::{GlobalArgs, OutputFormat, ReleasePrArgs, ReleasePrDecideArgs, ReleasePrVerifyArgs};
use crate::commands::read_json_arg;
use crate::error::CliError;
use crate::output::write_json;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: ReleasePrArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    match args {
        ReleasePrArgs::Decide(args) => decide(args, global),
        ReleasePrArgs::Verify(args) => verify(args, global),
    }
}

fn verify(args: ReleasePrVerifyArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let decision: ReleasePrDecisionV1 = serde_json::from_str(&read_json_arg(&args.decision)?)
        .map_err(|error| CliError::Other(format!("invalid release PR decision: {error}")))?;
    let snapshot: ReleasePrSnapshotV1 = serde_json::from_str(&read_json_arg(&args.snapshot)?)
        .map_err(|error| CliError::Other(format!("invalid release PR snapshot: {error}")))?;
    decision.verify_snapshot(&snapshot)?;
    match global.format {
        OutputFormat::Json => write_json(
            &mut std::io::stdout(),
            &serde_json::json!({"schemaVersion": 1, "ok": true}),
        )?,
        OutputFormat::Text => println!("release PR decision still matches forge snapshot"),
    }
    Ok(ExitCode::SUCCESS)
}

fn decide(args: ReleasePrDecideArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let snapshot: ReleasePrSnapshotV1 = serde_json::from_str(&read_json_arg(&args.snapshot)?)
        .map_err(|error| CliError::Other(format!("invalid release PR snapshot: {error}")))?;
    let config = ReleasePrConfigV1::new(args.repository, args.base_branch, args.release_branch)?;

    let runner = CliCommandRunner;
    let workspace = load_workspace(global, &runner)?;
    let status = status(&workspace, &StatusOptions::default())?;
    let decision = ReleasePrDecisionV1::derive(status.has_changesets, &config, &snapshot)?;

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &decision)?,
        OutputFormat::Text => render_decision(&decision),
    }
    Ok(ExitCode::SUCCESS)
}

fn render_decision(decision: &ReleasePrDecisionV1) {
    match &decision.action {
        ReleasePrActionV1::Noop { reason } => println!("release PR: no-op ({reason:?})"),
        ReleasePrActionV1::Create { branch } => println!("release PR: create {branch}"),
        ReleasePrActionV1::Update {
            pull_request_number,
            branch,
        } => println!("release PR: update #{pull_request_number} ({branch})"),
        ReleasePrActionV1::Supersede {
            pull_request_number,
            expected_branch,
            replacement_branch,
        } => println!("release PR: supersede #{pull_request_number} ({expected_branch}) with {replacement_branch}"),
        _ => println!("release PR: unsupported decision"),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::cli::{Cli, Command, ReleasePrArgs};

    #[test]
    fn cli_parses_release_pr_decide_with_explicit_snapshot_and_identity() {
        let cli = Cli::try_parse_from([
            "callisto",
            "release-pr",
            "decide",
            "--snapshot",
            "snapshot.json",
            "--repository",
            "orin-dx/callisto",
            "--base-branch",
            "main",
            "--release-branch",
            "callisto/version-packages",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::ReleasePr(ReleasePrArgs::Decide(_))));
    }

    #[test]
    fn cli_parses_release_pr_snapshot_verification() {
        let cli = Cli::try_parse_from([
            "callisto",
            "release-pr",
            "verify",
            "--decision",
            "decision.json",
            "--snapshot",
            "snapshot.json",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::ReleasePr(ReleasePrArgs::Verify(_))));
    }
}
