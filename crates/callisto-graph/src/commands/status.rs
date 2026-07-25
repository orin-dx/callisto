use callisto_model::{CommandRunner, StatusReport, Version, SCHEMA_VERSION};

use crate::changed::changed_since_last_tag;
use crate::commands::escalate;
use crate::error::GraphError;
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct StatusOptions {
    pub strict: bool,
    pub strict_graph: bool,
}

pub fn status<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    opts: &StatusOptions,
) -> Result<StatusReport, GraphError> {
    let mut packages = Vec::new();
    let base_versions = ws.base_versions()?;

    for pkg in ws.graph.packages() {
        let current_version = base_versions
            .get(&pkg.id)
            .cloned()
            .unwrap_or_else(|| Version::semver(1, 0, 0));
        let last_tag = ws.tags.last_tag(&pkg.id).map(|t| t.name.clone());
        let _changed = changed_since_last_tag(ws.runner, &ws.root, pkg, &ws.tags)?;

        packages.push(callisto_model::StatusPackageRecord {
            package: pkg.id.clone(),
            current_version,
            last_tag,
            pending_severity: None,
            pending_changesets: Vec::new(),
        });
    }

    let mut diagnostics = Vec::new();
    escalate(&mut diagnostics, opts.strict, opts.strict_graph);

    Ok(StatusReport {
        schema_version: SCHEMA_VERSION,
        packages,
        diagnostics,
    })
}
