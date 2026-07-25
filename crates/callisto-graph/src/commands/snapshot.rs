use callisto_model::{CommandRunner, SnapshotReport, SCHEMA_VERSION};

use crate::error::GraphError;
use crate::plan::VersionPlan;
use crate::resolver::DependencyResolver;
use crate::Workspace;

pub fn plan_snapshot<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    tag: &str,
) -> Result<(VersionPlan, SnapshotReport), GraphError> {
    let output = ws.runner.run("git", &["rev-parse", "HEAD"], &ws.root)?;
    let sha_raw = output.stdout_trimmed();
    let sha_short = if sha_raw.len() >= 7 {
        &sha_raw[..7]
    } else {
        "0000000"
    };

    let snapshot_tag = format!("0.0.0-{tag}-{sha_short}");

    let report = SnapshotReport {
        schema_version: SCHEMA_VERSION,
        snapshot_tag,
        bumps: Vec::new(),
        diagnostics: Vec::new(),
    };

    Ok((VersionPlan::default(), report))
}
