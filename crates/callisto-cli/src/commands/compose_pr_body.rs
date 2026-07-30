use std::io::Read;
use std::process::ExitCode;

use callisto_graph::commands::PrBodyOptions;
use callisto_graph::infer::NoInference;

use crate::cli::{ComposePrBodyArgs, GlobalArgs, OutputFormat};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: ComposePrBodyArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let existing_body = match args.existing_body {
        Some(ref s) if s == "-" => {
            let mut stdin_buf = String::new();
            std::io::stdin().read_to_string(&mut stdin_buf)?;
            let clean_stdin = stdin_buf
                .strip_prefix('\u{FEFF}')
                .unwrap_or(&stdin_buf)
                .to_string();
            Some(clean_stdin)
        }
        other => other,
    };

    let inference = NoInference;
    let opts = PrBodyOptions {
        existing_body,
        labels: args.labels,
        branch: args.branch,
    };

    let report = callisto_graph::commands::compose_pr_body(&ws, &inference, &opts)?;

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_compose_pr_body(&report, &mut std::io::stdout())?,
    }

    Ok(ExitCode::SUCCESS)
}
