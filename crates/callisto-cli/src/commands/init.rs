use std::process::ExitCode;

use callisto_graph::commands::InitOptions;

use crate::cli::{GlobalArgs, InitArgs, OutputFormat};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: InitArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let opts = InitOptions { yes: args.yes };

    let report = callisto_graph::commands::init(&ws, &opts)?;

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_init(&report, &mut std::io::stdout())?,
    }

    Ok(ExitCode::SUCCESS)
}
