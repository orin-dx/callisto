use std::fs;
use std::io::Read;
use std::process::ExitCode;

use callisto_model::{ApplyPermit, PublishPlan};

use crate::cli::{GlobalArgs, OutputFormat, TagArgs};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: TagArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let plan_text = if args.plan == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else if args.plan.trim_start().starts_with('{') {
        args.plan.clone()
    } else {
        fs::read_to_string(&args.plan).map_err(|source| CliError::Io {
            source,
            path: Some(std::path::PathBuf::from(&args.plan)),
        })?
    };

    let plan: PublishPlan = serde_json::from_str(&plan_text)
        .map_err(|e| CliError::Other(format!("Failed to parse publish plan: {e}")))?;

    let opts = callisto_graph::commands::TagOptions {
        floating_major: args.floating_major,
    };
    let permit = ApplyPermit::granted_unless_dry_run(global.dry_run);
    let report =
        callisto_graph::commands::create_tags_with_options(&ws, &plan, &opts, permit.as_ref())?;

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_tag(&report, &mut std::io::stdout())?,
    }

    Ok(ExitCode::SUCCESS)
}
