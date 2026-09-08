use std::process::ExitCode;

use callisto_graph::commands::PublishOptions;

use crate::cli::{GlobalArgs, OutputFormat, PlanPublishArgs};
use crate::error::CliError;
use crate::output::write_report_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

/// Loads the workspace and builds the publish plan -- the pipeline `publish`
/// and `plan-publish` share in full; they differ only in how they render the
/// result (and `publish` adds a dry-run banner and deprecation notice).
pub(crate) fn build_plan(global: &GlobalArgs, only: Vec<String>) -> Result<callisto_model::PublishPlan, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;
    let opts = PublishOptions { only };
    Ok(callisto_graph::commands::plan_publish(&ws, &opts)?)
}

pub fn handle(args: PlanPublishArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let report = build_plan(global, args.only)?;

    match global.format {
        OutputFormat::Json => write_report_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_publish(&report, &mut std::io::stdout())?,
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_json_format_succeeds_on_empty_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();

        let global = GlobalArgs {
            format: OutputFormat::Json,
            cwd: root.to_path_buf(),
            dry_run: false,
        };

        let result = handle(PlanPublishArgs::default(), &global);
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
    }

    #[test]
    fn handle_text_format_succeeds_on_empty_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
        std::fs::write(root.join("callisto.toml"), "").unwrap();

        let global = GlobalArgs {
            format: OutputFormat::Text,
            cwd: root.to_path_buf(),
            dry_run: false,
        };

        let result = handle(PlanPublishArgs::default(), &global);
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
    }
}
