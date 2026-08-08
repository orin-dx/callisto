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

/// Writes the dry-run text notice for the publish command to the given
/// writer. When the plan contains no publishable packages, emits a clear
/// "nothing to publish (dry run)" message; otherwise previews the plan.
pub(crate) fn write_dry_run_text<W: std::io::Write>(
    plan: &callisto_model::PublishPlan,
    w: &mut W,
) -> std::io::Result<()> {
    let is_empty = plan.rust_crates.is_empty()
        && plan.npm_main_packages.is_empty()
        && plan.npm_platform_packages.is_empty()
        && plan.pypi_packages.is_empty();
    if is_empty {
        writeln!(w, "No packages published (dry run).")?;
    } else {
        writeln!(
            w,
            "Dry run: about to publish the following plan (nothing will be published):"
        )?;
        render::render_publish(plan, w)?;
    }
    Ok(())
}

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
            OutputFormat::Text => write_dry_run_text(&plan, &mut std::io::stdout())?,
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

    if report.has_failures() {
        return Ok(ExitCode::FAILURE);
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::SCHEMA_VERSION;

    fn empty_plan() -> callisto_model::PublishPlan {
        callisto_model::PublishPlan {
            schema_version: SCHEMA_VERSION,
            rust_crates: vec![],
            npm_main_packages: vec![],
            npm_platform_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        }
    }

    /// Spec: when no packages would be published and --dry-run is active,
    /// the text output must contain "dry run" (case-insensitive) so the
    /// operator can see that nothing was published.
    #[test]
    fn dry_run_text_contains_dry_run_for_empty_plan() {
        let plan = empty_plan();
        let mut out = Vec::<u8>::new();
        write_dry_run_text(&plan, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.to_ascii_lowercase().contains("dry run"),
            "dry-run text output must contain 'dry run', got: {text:?}"
        );
    }
}
