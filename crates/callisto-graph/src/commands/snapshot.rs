use callisto_model::{CommandRunner, SnapshotReport, SCHEMA_VERSION};
use callisto_vcs::GitDataSource;

use crate::error::GraphError;
use crate::plan::VersionPlan;
use crate::resolver::DependencyResolver;
use crate::Workspace;

pub fn plan_snapshot<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    tag: &str,
) -> Result<(VersionPlan, SnapshotReport), GraphError> {
    // §G.11 (SPEC DECISION, pinned invariant #33): the sha component is a real, resolved
    // HEAD commit sha — never a fake placeholder. A resolution failure here must surface
    // as a real error rather than silently proceeding with a value that risks colliding
    // with snapshots from unrelated runs. Uses `GitAccess` (native gix, falling back to
    // the `CommandRunner` shell path when unavailable -- always true on wasm32) rather
    // than a direct `GitRepository::discover`, which has no such fallback and would
    // unconditionally fail on wasm32, hard-erroring `plan_snapshot` entirely there.
    let sha = callisto_vcs::GitAccess::discover(&ws.root, ws.runner).head_sha()?;
    let sha_short = sha.short();

    // Base is literally `0.0.0`, never the package's own version, and every package in
    // the workspace gets this identical, hyphen-joined string (§G.11 invariant #33) — not
    // a per-package, dot-joined prerelease of that package's real version.
    let snapshot_tag = format!("0.0.0-{tag}-{sha_short}");
    let snapshot_ver =
        callisto_model::Version::parse(&snapshot_tag, callisto_model::VersionGrammar::SemVer)
            .map_err(|_err| {
                GraphError::Bump(callisto_format::BumpError::NotSemVer {
                    raw: snapshot_tag.clone(),
                    grammar: callisto_model::VersionGrammar::SemVer,
                })
            })?;
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
            to: snapshot_ver.clone(),
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
        delete_pre_json: None,
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
