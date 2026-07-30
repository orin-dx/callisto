use callisto_model::{CommandRunner, SnapshotReport, SCHEMA_VERSION};

use crate::error::GraphError;
use crate::plan::VersionPlan;
use crate::resolver::DependencyResolver;
use crate::Workspace;

pub fn plan_snapshot<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    tag: &str,
) -> Result<(VersionPlan, SnapshotReport), GraphError> {
    let sha_raw = if let Ok(repo) = callisto_vcs::GitRepository::discover(&ws.root) {
        if let Ok(sha) = repo.head_sha() {
            sha.as_str().to_string()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let sha_short = if sha_raw.len() >= 7 {
        &sha_raw[..7]
    } else {
        "0000000"
    };

    let snapshot_tag = format!("0.0.0-{tag}-{sha_short}");
    let base_versions = ws.base_versions()?;
    let mut initial_severities = std::collections::BTreeMap::new();
    let mut initial_reasons = std::collections::BTreeMap::new();
    let mut initial_named_by = std::collections::BTreeMap::new();

    for pkg in ws.graph.packages() {
        initial_severities.insert(pkg.id.clone(), callisto_model::Severity::Patch);
        initial_reasons.insert(
            pkg.id.clone(),
            callisto_model::BumpReason::PreRelease {
                tag: tag.to_string(),
            },
        );
        initial_named_by.insert(pkg.id.clone(), crate::aggregate::NamedBy::Changeset);
    }

    let cascade_input = crate::cascade::CascadeInput {
        graph: &ws.graph,
        groups: &ws.config.groups,
        cfg: &ws.config.cascade,
        seed: &initial_severities,
        reasons: &initial_reasons,
        named_by: &initial_named_by,
        base: &base_versions,
        pre: None,
    };

    let cascade_out = crate::cascade::run_cascade(cascade_input)?;

    let mut bumps = Vec::new();
    let mut plan_bumps = Vec::new();

    let mut snapshot_versions = std::collections::BTreeMap::new();

    for pkg in ws.graph.packages() {
        let from = base_versions.get(&pkg.id).cloned().ok_or_else(|| {
            GraphError::Manifest(callisto_model::ManifestError::MissingField {
                path: pkg
                    .manifests
                    .first()
                    .map(|m| m.path.clone())
                    .unwrap_or_default(),
                field: "version",
            })
        })?;
        let snapshot_ver_str = format!("{}-{tag}.{sha_short}", from.render());
        let snapshot_ver = callisto_model::Version::parse(
            &snapshot_ver_str,
            callisto_model::VersionGrammar::SemVer,
        )
        .map_err(|_err| {
            GraphError::Bump(callisto_format::BumpError::NotSemVer {
                raw: snapshot_ver_str,
                grammar: callisto_model::VersionGrammar::SemVer,
            })
        })?;

        snapshot_versions.insert(pkg.id.clone(), snapshot_ver.clone());

        let mut writes = Vec::new();
        for decl in &pkg.manifests {
            if decl.role == callisto_model::ManifestRole::Canonical {
                writes.push(crate::plan::VersionWriteTarget::Manifest(decl.path.clone()));
            }
        }

        plan_bumps.push(crate::plan::PlannedBump {
            package: pkg.id.clone(),
            from: from.clone(),
            to: snapshot_ver.clone(),
            severity: callisto_model::Severity::Patch,
            governed_by: None,
            reason: None,
            writes,
        });

        bumps.push(callisto_model::BumpRecord {
            package: pkg.id.clone(),
            from,
            to: snapshot_ver,
            severity: callisto_model::Severity::Patch,
            governed_by: None,
            reason: None,
        });
    }

    let mut rewrites: Vec<_> = cascade_out.rewrites.into_values().collect();
    for rewrite in &mut rewrites {
        if let Some(snap_to) = snapshot_versions.get(&rewrite.dependency) {
            let eco = rewrite
                .dependency
                .ecosystem()
                .unwrap_or(callisto_model::Ecosystem::Cargo);
            match crate::cascade::rewrite_spec(&rewrite.from, snap_to, eco, &ws.config.cascade) {
                crate::cascade::RewriteOutcome::Rewritten(new_spec) => {
                    rewrite.to = new_spec;
                }
                _ => {
                    rewrite.to = callisto_model::DepSpec::Exact(snap_to.clone());
                }
            }
        }
    }

    let plan = VersionPlan {
        bumps: plan_bumps,
        rewrites,
        platform_writes: Vec::new(),
        optional_dep_updates: Vec::new(),
        changelog_writes: Vec::new(),
        consumed_changesets: Vec::new(),
        pre_state_update: None,
        delete_pre_json: false,
        pre_cursor_updates: Vec::new(),
        observed_versions: std::collections::BTreeMap::new(),
        diagnostics: cascade_out.diagnostics,
    };

    let report = SnapshotReport {
        schema_version: SCHEMA_VERSION,
        snapshot_tag,
        bumps,
        diagnostics: Vec::new(),
    };

    Ok((plan, report))
}
