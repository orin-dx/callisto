use std::process::ExitCode;

use callisto_graph::apply::{apply_version_plan, ApplyOptions};
use callisto_model::{ApplyPermit, DiagnosticSeverity};

use crate::cli::{GlobalArgs, OutputFormat, SnapshotArgs};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: SnapshotArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    // Under `--strict`, promote graph diagnostics (including crosscheck
    // failures) to Error severity and abort before touching any files.
    if args.strict {
        let mut diags = ws.graph.diagnostics().to_vec();
        callisto_graph::commands::escalate(&mut diags, true, true);
        let has_errors = diags
            .iter()
            .any(|d| d.severity == DiagnosticSeverity::Error);
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

    let (plan, report) = callisto_graph::commands::plan_snapshot(&ws, &args.tag)?;

    let apply_opts = ApplyOptions {
        refresh_lockfiles: false,
        transient: true,
    };

    if let Some(permit) = ApplyPermit::granted_unless_dry_run(global.dry_run) {
        apply_version_plan(&ws.root, &plan, &runner, &apply_opts, &permit)?;
    }

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => {
            if global.dry_run {
                println!("[DRY-RUN] Snapshot preview (no files modified):");
            }
            render::render_snapshot(&report, &mut std::io::stdout())?;
        }
    }

    Ok(ExitCode::SUCCESS)
}
