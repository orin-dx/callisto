use std::process::ExitCode;

use callisto_graph::apply::{apply_version_plan, ApplyOptions, ApplyOutcome};
use callisto_graph::commands::VersionOptions;
use callisto_graph::infer::NoInference;
use callisto_model::{ApplyPermit, DiagnosticSeverity};

use crate::cli::{GlobalArgs, OutputFormat, VersionArgs};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: VersionArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let inference = NoInference;
    let opts = VersionOptions {
        strict: args.strict,
        strict_graph: args.strict_graph,
        allow_empty_changesets: args.allow_empty_changesets,
    };

    let plan = callisto_graph::commands::plan_version(&ws, &inference, &opts)?;

    let apply_opts = ApplyOptions {
        refresh_lockfiles: args.refresh_lockfiles,
    };

    let outcome = match ApplyPermit::granted_unless_dry_run(global.dry_run) {
        Some(permit) => apply_version_plan(&ws.root, &plan, &runner, &apply_opts, &permit)?,
        None => ApplyOutcome::default(),
    };
    let report = plan.to_report(outcome.lockfile_refresh_results);

    if global.dry_run && global.format == OutputFormat::Text {
        println!("[DRY-RUN] Version Plan Calculated (no files modified):");
    }

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_version(&report, &mut std::io::stdout())?,
    }

    // If any diagnostic was escalated to Error (e.g. by --strict), fail.
    // Mirrors the pattern in status.rs: diagnostics ride in the report so the
    // caller sees full detail before the non-zero exit.
    let has_errors = report
        .diagnostics
        .iter()
        .any(|d| d.severity == DiagnosticSeverity::Error);

    if has_errors {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}
