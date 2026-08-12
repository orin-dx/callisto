use std::process::ExitCode;

use callisto_graph::commands::PublishOptions;

use crate::cli::{GlobalArgs, OutputFormat, PlanPublishArgs};
use crate::error::CliError;
use crate::output::write_report_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: PlanPublishArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let opts = PublishOptions { only: args.only };
    let report = callisto_graph::commands::plan_publish(&ws, &opts)?;

    match global.format {
        OutputFormat::Json => write_report_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_publish(&report, &mut std::io::stdout())?,
    }

    Ok(ExitCode::SUCCESS)
}
