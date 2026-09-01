use std::process::ExitCode;

use callisto_model::{DiagnosticSeverity, PublishPlan};

use crate::cli::{GlobalArgs, OutputFormat, TagArgs};
use crate::error::CliError;
use crate::output::write_json;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: TagArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    // Under `--strict`, promote graph diagnostics (including crosscheck
    // failures) to Error severity and abort before creating any tags.
    if args.strict {
        let mut diags = ws.graph.diagnostics().to_vec();
        callisto_graph::commands::escalate(&mut diags, true, true);
        let has_errors = diags.iter().any(|d| d.severity == DiagnosticSeverity::Error);
        if has_errors {
            let messages: Vec<String> = diags
                .iter()
                .filter(|d| d.severity == DiagnosticSeverity::Error)
                .map(|d| d.message.clone())
                .collect();
            return Err(CliError::Other(format!(
                "--strict: workspace graph has crosscheck failures:\n{}",
                messages.join("\n")
            )));
        }
    }

    let plan_text = crate::commands::read_json_arg(&args.plan)?;

    let plan: PublishPlan =
        serde_json::from_str(&plan_text).map_err(|e| CliError::Other(format!("Failed to parse publish plan: {e}")))?;

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &plan)?,
        OutputFormat::Text => {
            let _floating_major = args.floating_major;
            println!("Tag preview only; no Git tags were created. Use `callisto release execute` after validating a durable release intent.");
        }
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--plan <path>` (a value that is neither `-` nor inline JSON) must be
    /// read from disk rather than treated as a literal plan or a stdin marker.
    #[test]
    fn handle_reads_plan_from_a_file_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();
        drop(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(root)
                .output(),
        );

        let plan_path = root.join("plan.json");
        std::fs::write(
            &plan_path,
            serde_json::json!({
                "schemaVersion": 1,
                "rustCrates": [],
                "npmPlatformPackages": [],
                "npmMainPackages": [],
                "releases": []
            })
            .to_string(),
        )
        .unwrap();

        let global = GlobalArgs {
            format: OutputFormat::Json,
            cwd: root.to_path_buf(),
            dry_run: true,
        };

        let result = handle(
            TagArgs {
                plan: plan_path.to_string_lossy().to_string(),
                floating_major: false,
                strict: false,
            },
            &global,
        );
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
    }

    /// `--plan <path>` for a path that does not exist must surface a
    /// `CliError::Io` naming that path, not panic or propagate a bare
    /// `std::io::Error`.
    #[test]
    fn handle_reports_io_error_for_a_nonexistent_plan_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();
        drop(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(root)
                .output(),
        );

        let missing_plan = root.join("does-not-exist.json");
        let global = GlobalArgs {
            format: OutputFormat::Json,
            cwd: root.to_path_buf(),
            dry_run: true,
        };

        let result = handle(
            TagArgs {
                plan: missing_plan.to_string_lossy().to_string(),
                floating_major: false,
                strict: false,
            },
            &global,
        );
        match result {
            Err(CliError::Io { path, .. }) => {
                assert_eq!(path, Some(missing_plan));
            }
            other => panic!("expected CliError::Io, got: {other:?}"),
        }
    }
}
