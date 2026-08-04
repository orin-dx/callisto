use callisto_model::{CommandRunner, StatusReport, SCHEMA_VERSION};

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
    let loaded_changesets = crate::load_changesets(&ws.root, &ws.config)?;
    let tags = ws.tags()?;

    for pkg in ws.graph.packages() {
        let current_version = base_versions.get(&pkg.id).cloned().ok_or_else(|| {
            GraphError::Manifest(callisto_model::ManifestError::MissingField {
                path: pkg
                    .manifests
                    .first()
                    .map(|m| m.path.clone())
                    .unwrap_or_default(),
                field: "version",
            })
        })?;
        let last_tag = tags.last_tag(&pkg.id).map(|t| t.name.clone());
        let changed = changed_since_last_tag(ws.runner, &ws.root, pkg, tags)?;

        let mut pkg_changesets = Vec::new();
        let mut max_sev: Option<callisto_model::Severity> = None;

        for lc in &loaded_changesets {
            for entry in &lc.changeset.entries {
                if entry.name == pkg.id.to_string() {
                    let name = lc
                        .path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    pkg_changesets.push(name);
                    max_sev = match (max_sev, entry.severity) {
                        (None, s) => Some(s),
                        (Some(cur), s) => Some(cur.max(s)),
                    };
                }
            }
        }

        packages.push(callisto_model::StatusPackageRecord {
            package: pkg.id.clone(),
            current_version,
            last_tag,
            pending_severity: max_sev,
            changed_since_last_tag: changed,
            pending_changesets: pkg_changesets,
        });
    }

    let mut diagnostics = ws.graph.diagnostics().to_vec();
    escalate(&mut diagnostics, opts.strict, opts.strict_graph);

    Ok(StatusReport {
        schema_version: SCHEMA_VERSION,
        packages,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_status_package_record_structure() {
        let rec = callisto_model::StatusPackageRecord {
            package: callisto_model::PackageId::parse("test-pkg").unwrap(),
            current_version: callisto_model::Version::semver(1, 0, 0),
            last_tag: None,
            pending_severity: Some(callisto_model::Severity::Minor),
            changed_since_last_tag: false,
            pending_changesets: vec!["my-changeset".to_string()],
        };
        assert_eq!(rec.pending_changesets.len(), 1);
        assert_eq!(rec.pending_severity, Some(callisto_model::Severity::Minor));
    }
}
