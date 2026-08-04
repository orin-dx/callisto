use callisto_changelog::{ChangeSource, ChangelogEntry, ChangelogInput};
use callisto_model::{BumpReason, CommandRunner, GroupKind, Severity};

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

    let pre_path = ws.root.join(".changeset/pre.json");
    let pre_state = if pre_path.exists() {
        if let Ok(text) = std::fs::read_to_string(&pre_path) {
            callisto_format::parse_pre_json(&text).ok()
        } else {
            None
        }
    } else {
        None
    };

    let agg = aggregate(
        &ws.graph,
        &ws.config,
        ws.runner,
        ws.tags()?,
        &base_versions,
        pre_state.as_ref(),
        inference,
    )?;

    let input = CascadeInput {
        graph: &ws.graph,
        groups: &ws.config.groups,
        cfg: &ws.config.cascade,
        seed: &agg.severities,
        reasons: &agg.reasons,
        named_by: &agg.named_by,
        base: &base_versions,
        pre: pre_state.as_ref(),
    };

    let outcome = run_cascade(input)?;

    let mut bumps = Vec::new();
    let mut changelog_writes = Vec::new();

    for (id, &sev) in &outcome.severities {
        if sev == Severity::None {
            continue;
        }
        let pkg = ws.graph.packages().find(|p| &p.id == id).unwrap();
        let from = base_versions.get(id).cloned().ok_or_else(|| {
            GraphError::Manifest(callisto_model::ManifestError::MissingField {
                path: pkg
                    .manifests
                    .first()
                    .map(|m| m.path.clone())
                    .unwrap_or_default(),
                field: "version",
            })
        })?;
        let to = outcome.targets.get(id).cloned().ok_or_else(|| {
            GraphError::Manifest(callisto_model::ManifestError::MissingField {
                path: pkg
                    .manifests
                    .first()
                    .map(|m| m.path.clone())
                    .unwrap_or_default(),
                field: "version",
            })
        })?;

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
            let input = if let Some(mut agg_input) = agg.changelog_inputs.get(id).cloned() {
                // Real changeset data: use it, but set the resolved target version.
                agg_input.from = from.clone();
                agg_input.to = Some(to.clone());
                agg_input
            } else {
                // No changeset drove this bump (cascade, group, inference, pre-release).
                // Synthesize a single entry describing the reason so that render_section()
                // never receives an empty entries list (which returns EmptyInput).
                let source = match outcome.reasons.get(id) {
                    Some(BumpReason::FixedGroupUnion { group }) => ChangeSource::GroupUnion {
                        group: group.clone(),
                        kind: GroupKind::Fixed,
                    },
                    Some(BumpReason::LinkedGroupUnion { group }) => ChangeSource::GroupUnion {
                        group: group.clone(),
                        kind: GroupKind::Linked,
                    },
                    Some(BumpReason::Cascade {
                        via,
                        dep_kind,
                        dependency_to,
                        ..
                    }) => ChangeSource::DependencyUpdate {
                        dependency: via.clone(),
                        dep_kind: *dep_kind,
                        to: dependency_to.clone(),
                    },
                    Some(BumpReason::PeerEscalation { via, .. }) => {
                        let dep_to = outcome
                            .targets
                            .get(via)
                            .cloned()
                            .unwrap_or_else(|| to.clone());
                        ChangeSource::PeerEscalation {
                            dependency: via.clone(),
                            to: dep_to,
                        }
                    }
                    _ => ChangeSource::Changeset {
                        filename: String::new(),
                        summary: format!(
                            "Version bump ({})",
                            match sev {
                                Severity::Major => "major",
                                Severity::Minor => "minor",
                                Severity::Patch => "patch",
                                Severity::None => "none",
                            }
                        ),
                    },
                };
                ChangelogInput {
                    package: id.clone(),
                    from: from.clone(),
                    to: Some(to.clone()),
                    entries: vec![ChangelogEntry {
                        severity: sev,
                        source,
                    }],
                }
            };
            changelog_writes.push(crate::plan::ChangelogWrite {
                changelog_path: ch_path.clone(),
                input,
            });
        }
    }

    let mut diagnostics = outcome.diagnostics;
    if agg.consumed.is_empty()
        && !opts.allow_empty_changesets
        && !ws.config.validation.allow_empty_changesets
    {
        diagnostics.push(callisto_model::Diagnostic {
            code: callisto_model::DiagnosticCode::EmptyChangeset,
            severity: callisto_model::DiagnosticSeverity::Warning,
            message: "No pending changesets found in workspace".to_string(),
            package: None,
            path: None,
            escalated_by: Some(callisto_model::StrictFlag::Strict),
            governed_by: Some(callisto_model::ConfigKey::VALIDATION_ALLOW_EMPTY_CHANGESETS),
        });
    }

    escalate(&mut diagnostics, opts.strict, opts.strict_graph);

    let (pre_state_update, delete_pre_json) = if let Some(mut state) = pre_state {
        if state.mode == callisto_format::PreMode::Exit {
            (None, true)
        } else {
            // Update pre-release state changesets
            for cs in &agg.consumed {
                if let Some(name) = cs.file_name().and_then(|n| n.to_str()) {
                    let stem = name.strip_suffix(".md").unwrap_or(name).to_string();
                    if !state.changesets.contains(&stem) {
                        state.changesets.push(stem);
                    }
                }
            }
            (Some(state), false)
        }
    } else {
        (None, false)
    };

    Ok(VersionPlan {
        bumps,
        rewrites: outcome.rewrites.into_values().collect(),
        platform_writes: Vec::new(),
        optional_dep_updates: Vec::new(),
        changelog_writes,
        consumed_changesets: agg.consumed,
        pre_state_update,
        delete_pre_json,
        pre_cursor_updates: Vec::new(),
        observed_versions: base_versions,
        diagnostics,
    })
}
