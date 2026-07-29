use std::process::ExitCode;

use callisto_graph::commands::StatusOptions;
use callisto_model::DiagnosticSeverity;

use crate::cli::{GlobalArgs, OutputFormat, StatusArgs};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: StatusArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let opts = StatusOptions {
        strict: args.strict,
        strict_graph: args.strict_graph,
    };

    let report = callisto_graph::commands::status(&ws, &opts)?;

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_status(&report, &mut std::io::stdout())?,
    }

    let has_errors = report.diagnostics.iter().any(|d| {
        d.severity == DiagnosticSeverity::Error
            || (args.strict && d.severity == DiagnosticSeverity::Warning)
    });

    if has_errors {
        Ok(ExitCode::FAILURE)
    } else if args.check {
        let has_pending = report
            .packages
            .iter()
            .any(|p| !p.pending_changesets.is_empty());
        if has_pending {
            Ok(ExitCode::SUCCESS)
        } else {
            Ok(ExitCode::from(2))
        }
    } else {
        Ok(ExitCode::SUCCESS)
    }
}
