use std::process::ExitCode;

use callisto_graph::commands::ValidateOptions;

use crate::cli::{GlobalArgs, OutputFormat, ValidateArgs};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: ValidateArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let opts = ValidateOptions {
        staged: args.staged,
        since: args.since,
        strict: args.strict,
        strict_graph: args.strict_graph,
    };

    let report = callisto_graph::commands::validate(&ws, &opts)?;

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_validate(&report, &mut std::io::stdout())?,
    }

    if report.valid {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}
