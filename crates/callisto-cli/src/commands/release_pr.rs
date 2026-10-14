//! Interface for typed managed release-pull-request decisions, and for
//! building the exact forge commit-API file changes the executor submits.
//! `decide` and `verify` are read-only; `commit-plan` reads the Git index
//! and worktree but writes no forge state itself -- the executor script is
//! the only thing that ever calls GitHub.

use std::process::ExitCode;

use callisto_graph::commands::{status, StatusOptions};
use callisto_model::vcs::GitAccess;
use callisto_model::{
    ApplyPermit, CommitSha, GitHubRepository, ReleasePrActionV2, ReleasePrCommitPlanReport, ReleasePrCommitPlanV1,
    ReleasePrConfigV1, ReleasePrConfigWireV1, ReleasePrDecideReport, ReleasePrDecisionError, ReleasePrDecisionV2,
    ReleasePrDecisionWireV2, ReleasePrSnapshotV2, ReleasePrSnapshotWireV2, ReleasePrVerifyReport,
};

use crate::cli::{
    GlobalArgs, OutputFormat, ReleasePrArgs, ReleasePrCommitPlanArgs, ReleasePrDecideArgs, ReleasePrVerifyArgs,
};
use crate::commands::read_json_arg;
use crate::error::CliError;
use crate::output::{emit_line, emit_report};
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: ReleasePrArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    match args {
        ReleasePrArgs::Decide(args) => decide(args, global),
        ReleasePrArgs::Verify(args) => verify(args, global),
        ReleasePrArgs::CommitPlan(args) => commit_plan(args, global),
    }
}

/// Reads `flag`'s argument (file, inline JSON, or `-` for stdin) as an unvalidated wire
/// value. Kept separate from the domain type's own validating constructor so a JSON parse
/// failure (`E281`) and a domain validation failure (its own typed `E1xx` code) are never
/// folded into one generic error.
fn read_json_arg_as<T: serde::de::DeserializeOwned>(flag: &'static str, arg: &str) -> Result<T, CliError> {
    let text = read_json_arg(arg)?;
    serde_json::from_str(&text).map_err(|error| CliError::ReleasePrArgJsonInvalid {
        flag,
        detail: error.to_string(),
    })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleasePrDecisionInput {
    schema_version: u8,
    config: ReleasePrConfigWireV1,
    snapshot: ReleasePrSnapshotWireV2,
    action: ReleasePrActionV2,
    #[serde(default, deserialize_with = "deserialize_present")]
    command: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present")]
    dry_run: Option<bool>,
}

// Missing report metadata is allowed, but present null values are not.
fn deserialize_present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn read_decision_arg(arg: &str) -> Result<ReleasePrDecisionWireV2, CliError> {
    let input: ReleasePrDecisionInput = read_json_arg_as("decision", arg)?;
    match (&input.command, input.dry_run) {
        (None, None) => {}
        (Some(command), Some(_)) if command == <ReleasePrDecideReport as callisto_model::Report>::COMMAND => {}
        _ => {
            return Err(CliError::ReleasePrArgJsonInvalid {
                flag: "decision",
                detail: "report metadata must include command=\"release-pr decide\" and a boolean dryRun".into(),
            });
        }
    }
    Ok(ReleasePrDecisionWireV2 {
        schema_version: input.schema_version,
        config: input.config,
        snapshot: input.snapshot,
        action: input.action,
    })
}

fn verify(args: ReleasePrVerifyArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let decision_wire = read_decision_arg(&args.decision)?;
    let decision = ReleasePrDecisionV2::from_wire(decision_wire)?;
    let snapshot_wire: ReleasePrSnapshotWireV2 = read_json_arg_as("snapshot", &args.snapshot)?;
    let snapshot = ReleasePrSnapshotV2::from_wire(snapshot_wire)?;
    match args.applied_head {
        Some(head) => decision.verify_snapshot_after_update(&snapshot, &CommitSha::parse(&head)?)?,
        None => decision.verify_snapshot(&snapshot)?,
    }
    match global.format {
        OutputFormat::Json => emit_report(
            &mut std::io::stdout(),
            &ReleasePrVerifyReport { ok: true },
            global.dry_run,
        )?,
        OutputFormat::Text => emit_line("release PR decision still matches forge snapshot")?,
    }
    Ok(ExitCode::SUCCESS)
}

fn decide(args: ReleasePrDecideArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let snapshot_wire: ReleasePrSnapshotWireV2 = read_json_arg_as("snapshot", &args.snapshot)?;
    let snapshot = ReleasePrSnapshotV2::from_wire(snapshot_wire)?;
    let repository =
        GitHubRepository::parse(&args.repository).map_err(|_error| ReleasePrDecisionError::InvalidRepository {
            repository: args.repository.clone(),
        })?;
    let config = ReleasePrConfigV1::new(repository, args.base_branch, args.release_branch)?;

    let runner = CliCommandRunner;
    let workspace = load_workspace(global, &runner)?;
    let inference = crate::workspace::select_inference();
    let status = status(&workspace, &inference, &StatusOptions::default())?;
    let decision = ReleasePrDecisionV2::derive(status.has_changesets, &config, &snapshot)?;

    match global.format {
        OutputFormat::Json => emit_report(
            &mut std::io::stdout(),
            &ReleasePrDecideReport { decision },
            global.dry_run,
        )?,
        OutputFormat::Text => render_decision(&decision)?,
    }
    Ok(ExitCode::SUCCESS)
}

fn commit_plan(args: ReleasePrCommitPlanArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let base_commit = CommitSha::parse(&args.base_commit)?;

    let start = dunce::canonicalize(&global.cwd).map_err(|source| CliError::Io {
        source,
        path: Some(global.cwd.clone()),
    })?;
    let runner = CliCommandRunner;
    let git = GitAccess::new(&start, &runner);
    let changes = git.staged_changes_since(&base_commit)?;
    let plan = ReleasePrCommitPlanV1::from_changes(base_commit, args.message, changes)?;

    match args.out {
        Some(path) => {
            let permit =
                ApplyPermit::granted_unless_dry_run(global.dry_run).ok_or(CliError::ReleasePrCommitPlanDryRun)?;
            let content = serde_json::to_string_pretty(&plan).expect("commit plan serializes") + "\n";
            callisto_model::atomic::atomic_write(&path, &content, &permit).map_err(|source| CliError::Io {
                source,
                path: Some(path),
            })?;
        }
        None => emit_report(
            &mut std::io::stdout(),
            &ReleasePrCommitPlanReport { plan },
            global.dry_run,
        )?,
    }
    Ok(ExitCode::SUCCESS)
}

fn render_decision(decision: &ReleasePrDecisionV2) -> Result<(), CliError> {
    match &decision.action {
        ReleasePrActionV2::Noop { reason } => emit_line(&format!("release PR: no-op ({reason:?})"))?,
        ReleasePrActionV2::Create { branch, staging_branch } => {
            emit_line(&format!("release PR: create {branch} (staging {staging_branch})"))?
        }
        ReleasePrActionV2::Update {
            pull_request_number,
            branch,
            expected_head_commit,
            staging_branch,
        } => emit_line(&format!(
            "release PR: update #{pull_request_number} ({branch}, expected head {}, staging {staging_branch})",
            expected_head_commit.as_str()
        ))?,
        _ => emit_line("release PR: unsupported decision")?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{read_decision_arg, read_json_arg_as};
    use crate::cli::{Cli, Command, ReleasePrArgs};
    use crate::error::CliError;

    fn decision_json() -> serde_json::Value {
        serde_json::json!({
            "schemaVersion": 2,
            "config": {"schemaVersion": 1, "repository": "orin-dx/callisto",
                "baseBranch": "main", "releaseBranch": "callisto/version-packages"},
            "snapshot": {"schemaVersion": 2, "repository": "orin-dx/callisto",
                "baseBranch": "main", "baseCommit": "a".repeat(40), "openPullRequests": []},
            "action": {"kind": "create", "branch": "callisto/version-packages",
                "stagingBranch": "callisto/version-packages--staging"}
        })
    }

    #[test]
    fn decision_reader_accepts_bare_and_report_shapes() {
        let mut json = decision_json();
        read_decision_arg(&json.to_string()).unwrap();
        json["command"] = serde_json::json!("release-pr decide");
        for dry_run in [false, true] {
            json["dryRun"] = serde_json::json!(dry_run);
            let wire = read_decision_arg(&json.to_string()).unwrap();
            callisto_model::ReleasePrDecisionV2::from_wire(wire).unwrap();
        }
    }

    #[test]
    fn decision_reader_rejects_invalid_metadata_and_unknown_fields() {
        for (command, dry_run) in [
            (serde_json::json!("other"), serde_json::json!(false)),
            (serde_json::Value::Null, serde_json::json!(false)),
            (serde_json::json!(1), serde_json::json!(false)),
            (serde_json::json!("release-pr decide"), serde_json::Value::Null),
            (serde_json::json!("release-pr decide"), serde_json::json!("false")),
        ] {
            let mut json = decision_json();
            json["command"] = command;
            json["dryRun"] = dry_run;
            assert!(matches!(
                read_decision_arg(&json.to_string()),
                Err(CliError::ReleasePrArgJsonInvalid { .. })
            ));
        }
        for field in ["command", "dryRun", "unexpected"] {
            let mut json = decision_json();
            json[field] = match field {
                "command" => serde_json::json!("release-pr decide"),
                _ => serde_json::json!(false),
            };
            assert!(matches!(
                read_decision_arg(&json.to_string()),
                Err(CliError::ReleasePrArgJsonInvalid { .. })
            ));
        }
    }

    #[test]
    fn decision_reader_rejects_duplicate_payload_and_metadata() {
        for (field, value) in [
            ("schemaVersion", "2"),
            ("command", "\"release-pr decide\""),
            ("dryRun", "false"),
        ] {
            let mut json = decision_json();
            json["command"] = serde_json::json!("release-pr decide");
            json["dryRun"] = serde_json::json!(false);
            let text = format!("{{\"{field}\":{value},{}", &json.to_string()[1..]);
            assert!(matches!(
                read_decision_arg(&text),
                Err(CliError::ReleasePrArgJsonInvalid { .. })
            ));
        }
    }

    #[test]
    fn decision_reader_preserves_typed_domain_validation() {
        let mut json = decision_json();
        json["config"]["releaseBranch"] = serde_json::json!("bad..branch");
        let wire = read_decision_arg(&json.to_string()).unwrap();
        assert!(matches!(
            callisto_model::ReleasePrDecisionV2::from_wire(wire),
            Err(callisto_model::ReleasePrDecisionError::InvalidBranch { .. })
        ));
    }

    /// Malformed inline JSON on a `release-pr` argument must surface as the typed
    /// `ReleasePrArgJsonInvalid` (`E281`) naming the offending flag, not a bare `serde_json`
    /// error nor `CliError::Other`.
    #[test]
    fn read_json_arg_as_reports_typed_error_for_malformed_json() {
        let err = read_json_arg_as::<serde_json::Value>("snapshot", "{not json").unwrap_err();
        match err {
            CliError::ReleasePrArgJsonInvalid { flag, detail } => {
                assert_eq!(flag, "snapshot");
                assert!(!detail.is_empty());
            }
            other => panic!("expected ReleasePrArgJsonInvalid, got: {other:?}"),
        }
    }

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
