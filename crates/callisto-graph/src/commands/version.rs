use callisto_changelog::{ChangelogEntry, ChangelogInput};
use callisto_model::{CommandRunner, Severity, Version};

use crate::aggregate::aggregate;
use crate::cascade::{run_cascade, CascadeInput};
use crate::commands::escalate;
use crate::error::GraphError;
use crate::infer::SeverityInference;
use crate::plan::{PlannedBump, VersionPlan, VersionWriteTarget};
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct VersionOptions {
    pub strict: bool,
    pub strict_graph: bool,
    pub allow_empty_changesets: bool,
}

pub fn plan_version<R: CommandRunner, D: DependencyResolver, I: SeverityInference>(
    ws: &Workspace<'_, R, D>,
    inference: &I,
    opts: &VersionOptions,
) -> Result<VersionPlan, GraphError> {
    let base_versions = ws.base_versions()?;
    let agg = aggregate(&ws.graph, &ws.config, ws.runner, &ws.tags, None, inference)?;

    let input = CascadeInput {
        graph: &ws.graph,
        groups: &ws.config.groups,
        cfg: &ws.config.cascade,
        seed: &agg.severities,
        reasons: &agg.reasons,
        named_by: &agg.named_by,
        base: &base_versions,
        pre: None,
    };

    let outcome = run_cascade(input)?;

    let mut bumps = Vec::new();
    let mut changelog_writes = Vec::new();

    for (id, &sev) in &outcome.severities {
        if sev == Severity::None {
            continue;
        }
        let from = base_versions
            .get(id)
            .cloned()
            .unwrap_or_else(|| Version::semver(1, 0, 0));
        let to = outcome
            .targets
            .get(id)
            .cloned()
            .unwrap_or_else(|| Version::semver(1, 0, 0));
        let pkg = ws.graph.packages().find(|p| &p.id == id).unwrap();

        let mut writes = Vec::new();
        for decl in &pkg.manifests {
            if decl.role == callisto_model::ManifestRole::Canonical {
                writes.push(VersionWriteTarget::Manifest(decl.path.clone()));
            }
        }

        bumps.push(PlannedBump {
            package: id.clone(),
            from: from.clone(),
            to: to.clone(),
            severity: sev,
            governed_by: outcome.governed_by.get(id).cloned(),
            reason: outcome.reasons.get(id).cloned(),
            writes,
        });

        if let Some(ch_path) = &pkg.changelog {
            changelog_writes.push(crate::plan::ChangelogWrite {
                changelog_path: ch_path.clone(),
                input: ChangelogInput {
                    package: id.clone(),
                    from: from.clone(),
                    to: Some(to.clone()),
                    entries: vec![ChangelogEntry {
                        severity: sev,
                        source: callisto_changelog::ChangeSource::Changeset {
                            filename: "changeset.md".to_string(),
                            summary: "Release update".to_string(),
                        },
                    }],
                },
            });
        }
    }

    let mut diagnostics = outcome.diagnostics;
    escalate(&mut diagnostics, opts.strict, opts.strict_graph);

    Ok(VersionPlan {
        bumps,
        rewrites: outcome.rewrites.into_values().collect(),
        platform_writes: Vec::new(),
        optional_dep_updates: Vec::new(),
        changelog_writes,
        consumed_changesets: agg.consumed,
        pre_state_update: None,
        delete_pre_json: false,
        pre_cursor_updates: Vec::new(),
        observed_versions: base_versions,
        diagnostics,
    })
}
