use std::process::ExitCode;

use callisto_graph::apply::{apply_version_plan, ApplyOptions};
use callisto_model::ApplyPermit;

use crate::cli::{GlobalArgs, OutputFormat, SnapshotArgs};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

pub fn handle(args: SnapshotArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let (plan, report) = callisto_graph::commands::plan_snapshot(&ws, &args.tag)?;

    let apply_opts = ApplyOptions {
        refresh_lockfiles: false,
    };

    if let Some(permit) = ApplyPermit::granted_unless_dry_run(global.dry_run) {
        apply_version_plan(&ws.root, &plan, &runner, &apply_opts, &permit)?;
    }

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_snapshot(&report, &mut std::io::stdout())?,
    }

    Ok(ExitCode::SUCCESS)
}
