use std::process::ExitCode;

use callisto_graph::apply::{apply_version_plan, ApplyOptions};
use callisto_graph::commands::VersionOptions;
use callisto_graph::infer::NoInference;

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
        transient: false,
    };

    let outcome = apply_version_plan(&ws.root, &plan, &runner, &apply_opts)?;
    let report = plan.to_report(outcome.lockfile_refresh_results);

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_version(&report, &mut std::io::stdout())?,
    }

    Ok(ExitCode::SUCCESS)
}
