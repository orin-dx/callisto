use std::process::ExitCode;

use callisto_graph::commands::filter_plan_by_report;
use callisto_model::{PublishPlan, PublishReport};

use crate::cli::{FilterPlanArgs, GlobalArgs, OutputFormat};
use crate::commands::read_json_arg;
use crate::error::CliError;
use crate::output::write_report_json;
use crate::render;

/// Filters a publish plan down to the entries a publish report confirms
/// actually succeeded (`Published` or `AlreadyPublished`), dropping
/// anything that failed. Lets a CI pipeline that runs `plan-publish` ->
/// `publish` -> `tag`/`gh release create` as separate steps operate the
/// latter two on what actually shipped, instead of the pre-publish plan --
/// so one package's failure doesn't cost its already-succeeded siblings a
/// tag or a GitHub Release in the same run.
pub fn handle(args: FilterPlanArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let plan_text = read_json_arg(&args.plan)?;
    let plan: PublishPlan =
        serde_json::from_str(&plan_text).map_err(|e| CliError::Other(format!("Failed to parse publish plan: {e}")))?;

    let report_text = read_json_arg(&args.report)?;
    let report: PublishReport = serde_json::from_str(&report_text)
        .map_err(|e| CliError::Other(format!("Failed to parse publish report: {e}")))?;

    let filtered = filter_plan_by_report(&plan, &report);

    match global.format {
        OutputFormat::Json => write_report_json(&mut std::io::stdout(), &filtered)?,
        OutputFormat::Text => render::render_publish(&filtered, &mut std::io::stdout())?,
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn global(format: OutputFormat) -> GlobalArgs {
        GlobalArgs {
            format,
            cwd: std::env::temp_dir(),
            dry_run: false,
        }
    }

    fn plan_json() -> String {
        serde_json::json!({
            "schemaVersion": 1,
            "rustCrates": [{
                "name": "test-crate",
                "version": "1.0.0",
                "publishTo": "cratesIo",
            }],
            "npmPlatformPackages": [],
            "npmMainPackages": [],
            "releases": [],
        })
        .to_string()
    }

    fn report_json_all_published() -> String {
        serde_json::json!({
            "schemaVersion": 1,
            "attempts": [{
                "package": "cargo:test-crate",
                "version": "1.0.0",
                "status": "published",
            }],
        })
        .to_string()
    }

    fn report_json_failed() -> String {
        serde_json::json!({
            "schemaVersion": 1,
            "attempts": [{
                "package": "cargo:test-crate",
                "version": "1.0.0",
                "status": "failed",
                "errorKind": "other",
                "error": "boom",
            }],
        })
        .to_string()
    }

    #[test]
    fn handle_filters_plan_by_report_and_prints_json() {
        let args = FilterPlanArgs {
            plan: plan_json(),
            report: report_json_all_published(),
        };
        let result = handle(args, &global(OutputFormat::Json));
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert_eq!(result.unwrap(), ExitCode::SUCCESS);
    }

    #[test]
    fn handle_drops_failed_entries() {
        // Verified indirectly via filter_plan_by_report's own unit tests in
        // callisto-graph; this just confirms the CLI plumbing round-trips
        // inline JSON for both --plan and --report without erroring.
        let args = FilterPlanArgs {
            plan: plan_json(),
            report: report_json_failed(),
        };
        let result = handle(args, &global(OutputFormat::Text));
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
    }

    #[test]
    fn handle_reads_plan_and_report_from_file_paths() {
        let tmp = tempfile::TempDir::new().unwrap();
        let plan_path = tmp.path().join("plan.json");
        let report_path = tmp.path().join("report.json");
        std::fs::write(&plan_path, plan_json()).unwrap();
        std::fs::write(&report_path, report_json_all_published()).unwrap();

        let args = FilterPlanArgs {
            plan: plan_path.to_string_lossy().to_string(),
            report: report_path.to_string_lossy().to_string(),
        };
        let result = handle(args, &global(OutputFormat::Json));
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
    }

    #[test]
    fn handle_reports_parse_error_for_malformed_plan_json() {
        let args = FilterPlanArgs {
            plan: "{not valid json".to_string(),
            report: report_json_all_published(),
        };
        let result = handle(args, &global(OutputFormat::Json));
        assert!(result.is_err(), "malformed plan JSON must error, not silently succeed");
    }

    #[test]
    fn handle_reports_parse_error_for_malformed_report_json() {
        let args = FilterPlanArgs {
            plan: plan_json(),
            report: "{not valid json".to_string(),
        };
        let result = handle(args, &global(OutputFormat::Json));
        assert!(
            result.is_err(),
            "malformed report JSON must error, not silently succeed"
        );
    }
}
