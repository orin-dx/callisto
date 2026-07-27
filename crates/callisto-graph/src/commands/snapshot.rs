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

    for pkg in ws.graph.packages() {
        let from = base_versions
            .get(&pkg.id)
            .cloned()
            .unwrap_or_else(|| callisto_model::Version::semver(1, 0, 0));
        let snapshot_ver = callisto_model::Version::parse(
            &format!("{}-{tag}.{sha_short}", from.render()),
            callisto_model::VersionGrammar::SemVer,
        )
        .unwrap_or_else(|_| from.clone());

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

    let plan = VersionPlan {
        bumps: plan_bumps,
        rewrites: cascade_out.rewrites.into_values().collect(),
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
