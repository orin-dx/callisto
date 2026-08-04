use std::process::ExitCode;

use callisto_graph::commands::{
    plan_publish, AlwaysRetryPolicy, PublishOptions, PublishOrchestrator, SubprocessRegistryClient,
    SystemTimeProvider,
};

use callisto_model::ApplyPermit;

use crate::cli::{GlobalArgs, OutputFormat, PublishArgs};
use crate::error::CliError;
use crate::output::write_json;
use crate::render;
use crate::runner::CliCommandRunner;
use crate::workspace::load_workspace;

/// Publishes every package in the workspace's publish plan to its ecosystem
/// registry (crates.io, npm, PyPI) by shelling out to that ecosystem's own
/// publisher CLI (`cargo publish`, `npm publish`, `twine upload`) — never by
/// talking to a registry's HTTP API directly.
///
/// Respects the global `--dry-run` flag: when set, this only computes and
/// prints the plan that WOULD be published (the same plan `plan-publish`
/// reports) and returns without constructing a [`PublishOrchestrator`] or
/// running any publisher command.
pub fn handle(_args: PublishArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let opts = PublishOptions::default();
    let plan = plan_publish(&ws, &opts)?;

    let Some(permit) = ApplyPermit::granted_unless_dry_run(global.dry_run) else {
        match global.format {
            OutputFormat::Json => write_json(&mut std::io::stdout(), &plan)?,
            OutputFormat::Text => {
                println!(
                    "Dry run: about to publish the following plan (nothing will be published):"
                );
                render::render_publish(&plan, &mut std::io::stdout())?;
            }
        }
        return Ok(ExitCode::SUCCESS);
    };

    let client = SubprocessRegistryClient::new(CliCommandRunner, ws.root.clone());
    let orchestrator = PublishOrchestrator::new(client, AlwaysRetryPolicy, SystemTimeProvider);
    let report = orchestrator.execute(&plan, &permit);

    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &report)?,
        OutputFormat::Text => render::render_publish_report(&report, &mut std::io::stdout())?,
    }

    Ok(ExitCode::SUCCESS)
}
