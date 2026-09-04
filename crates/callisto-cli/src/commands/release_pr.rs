//! Interface for typed managed release-pull-request decisions, and for
//! building the exact forge commit-API file changes the executor submits.
//! `decide` and `verify` are read-only; `commit-plan` reads the Git index
//! and worktree but writes no forge state itself -- the executor script is
//! the only thing that ever calls GitHub.

use std::process::ExitCode;

use callisto_graph::commands::{status, StatusOptions};
use callisto_model::{
    ApplyPermit, CommitSha, ReleasePrActionV2, ReleasePrCommitPlanV1, ReleasePrConfigV1, ReleasePrDecisionV2,
    ReleasePrSnapshotV2,
};
use callisto_vcs::GitAccess;

use crate::cli::{
    GlobalArgs, OutputFormat, ReleasePrArgs, ReleasePrCommitPlanArgs, ReleasePrDecideArgs, ReleasePrVerifyArgs,
};
use crate::commands::read_json_arg;
use crate::error::CliError;
use crate::output::write_json;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: ReleasePrArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    match args {
        ReleasePrArgs::Decide(args) => decide(args, global),
        ReleasePrArgs::Verify(args) => verify(args, global),
        ReleasePrArgs::CommitPlan(args) => commit_plan(args, global),
    }
}

fn verify(args: ReleasePrVerifyArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let decision: ReleasePrDecisionV2 = serde_json::from_str(&read_json_arg(&args.decision)?)
        .map_err(|error| CliError::Other(format!("invalid release PR decision: {error}")))?;
    let snapshot: ReleasePrSnapshotV2 = serde_json::from_str(&read_json_arg(&args.snapshot)?)
        .map_err(|error| CliError::Other(format!("invalid release PR snapshot: {error}")))?;
    decision.verify_snapshot(&snapshot)?;
    match global.format {
        OutputFormat::Json => write_json(
            &mut std::io::stdout(),
            &serde_json::json!({"schemaVersion": ReleasePrDecisionV2::SCHEMA_VERSION, "ok": true}),
        )?,
        OutputFormat::Text => println!("release PR decision still matches forge snapshot"),
    }
    Ok(ExitCode::SUCCESS)
}

fn decide(args: ReleasePrDecideArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let snapshot: ReleasePrSnapshotV2 = serde_json::from_str(&read_json_arg(&args.snapshot)?)
        .map_err(|error| CliError::Other(format!("invalid release PR snapshot: {error}")))?;
    let config = ReleasePrConfigV1::new(args.repository, args.base_branch, args.release_branch)?;

    let runner = CliCommandRunner;
    let workspace = load_workspace(global, &runner)?;
    let status = status(&workspace, &StatusOptions::default())?;
    let decision = ReleasePrDecisionV2::derive(status.has_changesets, &config, &snapshot)?;

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &decision)?,
        OutputFormat::Text => render_decision(&decision),
    }
    Ok(ExitCode::SUCCESS)
}

fn commit_plan(args: ReleasePrCommitPlanArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let base_commit = CommitSha::parse(&args.base_commit)
        .map_err(|error| CliError::Other(format!("invalid --base-commit: {error}")))?;

    let start = dunce::canonicalize(&global.cwd).map_err(|source| CliError::Io {
        source,
        path: Some(global.cwd.clone()),
    })?;
    let runner = CliCommandRunner;
    let git = GitAccess::discover(&start, &runner);
    let changes = git.staged_changes_since(&base_commit)?;
    let plan = ReleasePrCommitPlanV1::from_changes(base_commit, args.message, changes)?;

    match args.out {
        Some(path) => {
            let permit = ApplyPermit::granted_unless_dry_run(global.dry_run).ok_or_else(|| {
                CliError::Other("release-pr commit-plan cannot write --out with --dry-run; omit --out".to_string())
            })?;
            let content = serde_json::to_string_pretty(&plan).expect("commit plan serializes") + "\n";
            callisto_model::atomic::atomic_write(&path, &content, &permit).map_err(|source| CliError::Io {
                source,
                path: Some(path),
            })?;
        }
        None => write_json(&mut std::io::stdout(), &plan)?,
    }
    Ok(ExitCode::SUCCESS)
}

fn render_decision(decision: &ReleasePrDecisionV2) {
    match &decision.action {
        ReleasePrActionV2::Noop { reason } => println!("release PR: no-op ({reason:?})"),
        ReleasePrActionV2::Create { branch, staging_branch } => {
            println!("release PR: create {branch} (staging {staging_branch})")
        }
        ReleasePrActionV2::Update {
            pull_request_number,
            branch,
            expected_head_commit,
            staging_branch,
        } => println!(
            "release PR: update #{pull_request_number} ({branch}, expected head {}, staging {staging_branch})",
            expected_head_commit.as_str()
        ),
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

    #[test]
    fn cli_parses_release_pr_commit_plan() {
        let cli = Cli::try_parse_from([
            "callisto",
            "release-pr",
            "commit-plan",
            "--base-commit",
            &"a".repeat(40),
            "--message",
            "chore(release): version packages",
            "--out",
            "plan.json",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::ReleasePr(ReleasePrArgs::CommitPlan(_))));
    }
}
