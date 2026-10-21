use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use callisto_model::{
    BumpReason, ConfigKey, Coverage, DepEdge, DepKind, DepLocator, DepSpec, Diagnostic, DiagnosticCode,
    DiagnosticSeverity, Ecosystem, GrammarMismatch, PackageId, Severity, Version,
};

use crate::config::GroupTable;
use crate::config::{CascadeConfig, CascadeMode};
use crate::error::GraphError;
use crate::identity::IdentityIndex;
use crate::resolver::DependencyResolver;
use crate::tags::TagIndex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CascadeDecision {
    pub severity: Severity,
    pub rewrite: bool,
    pub governed_by: Option<ConfigKey>,
    pub escalated: bool,
    pub unknown_coverage: bool,
}

pub fn cascade_action(kind: DepKind, coverage: Coverage, source: Severity, cfg: &CascadeConfig) -> CascadeDecision {
    use Coverage::*;
    use DepKind::*;

    let effective = match (cfg.mode, coverage) {
        (CascadeMode::Always, _) => DoesNotCover,
        (CascadeMode::OutOfRange, Covers) => Covers,
        (CascadeMode::OutOfRange, DoesNotCover) => DoesNotCover,
        (CascadeMode::OutOfRange, Unknown) => Covers,
    };

    let rewrite = matches!(coverage, DoesNotCover);

    let (severity, governed_by, escalated) = match (kind, effective) {
        (Runtime | Optional | Build, Covers) => (Severity::None, None, false),
        (Runtime | Optional | Build, DoesNotCover) => (
            cfg.bump_severity.as_severity(),
            Some(ConfigKey::CASCADE_BUMP_SEVERITY),
            false,
        ),
        (Peer, Covers) => (Severity::None, None, false),
        (Peer, DoesNotCover)
            if cfg.peer_escalation
                && matches!(coverage, DoesNotCover)
                && matches!(source, Severity::Minor | Severity::Major) =>
        {
            (Severity::Major, Some(ConfigKey::CASCADE_PEER_ESCALATION), true)
        }
        (Peer, DoesNotCover) => (
            cfg.bump_severity.as_severity(),
            Some(ConfigKey::CASCADE_BUMP_SEVERITY),
            false,
        ),
        (Dev, _) => (Severity::None, None, false),
        _ => (Severity::None, None, false),
    };

    let governed_by = match (cfg.mode, coverage, severity) {
        (CascadeMode::Always, Covers | Unknown, s) if s != Severity::None => Some(ConfigKey::CASCADE_MODE),
        _ => governed_by,
    };

    CascadeDecision {
        severity,
        rewrite,
        governed_by,
        escalated,
        unknown_coverage: matches!(coverage, Unknown),
    }
}

pub fn coverage(spec: &DepSpec, new: &Version) -> Result<Coverage, GrammarMismatch> {
    match spec {
        DepSpec::Exact(v) => {
            if v == new {
                Ok(Coverage::Covers)
            } else {
                Ok(Coverage::DoesNotCover)
            }
        }
        DepSpec::CargoBare(v) => {
            if caret_covers(v, new)? {
                Ok(Coverage::Covers)
            } else {
                Ok(Coverage::DoesNotCover)
            }
        }
        DepSpec::Range(req, _) => {
            if req.matches(new)? {
                Ok(Coverage::Covers)
            } else {
                Ok(Coverage::DoesNotCover)
            }
        }
        DepSpec::Workspace(_) => Ok(Coverage::Covers),
        DepSpec::WorkspaceProtocol(workspace) => workspace.coverage(new),
        DepSpec::Catalog(_) | DepSpec::Opaque(_) => Ok(Coverage::Unknown),
    }
}

pub(crate) fn caret_covers(cur: &Version, new: &Version) -> Result<bool, GrammarMismatch> {
    let cmp = Version::compare(new, cur)?;
    if cmp.is_lt() {
        return Ok(false);
    }
    // Pre-release versions are never covered by a caret range: ^1.2.3 must not
    // satisfy 1.9.0-alpha.1 even though 1.9.0-alpha.1 > 1.2.3 and they share
    // the same major. Caret ranges cover stable releases within the allowed
    // major/minor bounds only; a pre-release of an in-range stable version is
    // not itself a stable release and must not be treated as covered.
    if new.is_prerelease() {
        return Ok(false);
    }
    let cur_maj = cur.major().unwrap_or(0);
    let cur_min = cur.minor().unwrap_or(0);
    let new_maj = new.major().unwrap_or(0);
    let new_min = new.minor().unwrap_or(0);

    if cur_maj > 0 {
        Ok(new_maj == cur_maj)
    } else if cur_min > 0 {
        Ok(new_maj == 0 && new_min == cur_min)
    } else {
        Ok(new == cur)
    }
}

pub struct CascadeInput<'a, D: DependencyResolver> {
    pub graph: &'a D,
    pub groups: &'a GroupTable,
    pub cfg: &'a CascadeConfig,
    pub seed: &'a BTreeMap<PackageId, Severity>,
    pub reasons: &'a BTreeMap<PackageId, BumpReason>,
    pub named_by: &'a BTreeMap<PackageId, crate::aggregate::NamedBy>,
    pub base: &'a BTreeMap<PackageId, Version>,
    pub pre: Option<&'a callisto_model::format::PreState>,
    pub tags: &'a TagIndex,
    pub identity: &'a IdentityIndex,
}

#[derive(Clone, Debug, Default)]
pub struct CascadeOutcome {
    pub severities: BTreeMap<PackageId, Severity>,
    pub targets: BTreeMap<PackageId, Version>,
    pub reasons: BTreeMap<PackageId, BumpReason>,
    pub governed_by: BTreeMap<PackageId, ConfigKey>,
    pub rewrites: BTreeMap<RewriteKey, SpecRewrite>,
    pub diagnostics: Vec<Diagnostic>,
    pub iterations: usize,
}

pub fn run_cascade<D: DependencyResolver>(input: CascadeInput<'_, D>) -> Result<CascadeOutcome, GraphError> {
    solve_cascade(input)
}

pub(crate) fn manifest_ecosystem(from_manifest: &std::path::Path) -> Ecosystem {
    callisto_model::ManifestFormat::from_path(from_manifest)
        .map(|fmt| fmt.ecosystem())
        .unwrap_or(Ecosystem::Npm)
}

pub(crate) fn dependency_rewrite_key(edge: &DepEdge) -> RewriteKey {
    RewriteKey {
        target: if edge.inherited {
            DepWriteTarget::CargoWorkspaceDependency {
                root_manifest: edge.from_manifest.clone(),
            }
        } else {
            DepWriteTarget::Manifest(edge.from_manifest.clone())
        },
        name: edge.name.clone(),
        kind: if edge.inherited { None } else { Some(edge.kind) },
        // An inherited declaration is rewritten once, in the workspace root, whichever member table names it.
        locator: if edge.inherited {
            DepLocator::default()
        } else {
            edge.locator.clone()
        },
    }
}

/// Propagates `input.seed`'s severities outward to dependents until no
/// target version changes, or `convergence_bound`'s iteration cap is
/// exceeded ([`GraphError::CascadeNotConverged`]) -- turns a would-be
/// infinite loop into a reportable bug, bound derived from package count.
///
/// One pass: pop a package, compute its bumped target, walk every
/// dependent edge, apply [`cascade_action`] to decide if that dependent's
/// severity/target changes and gets re-queued.
///
/// A genuine cross-grammar coverage failure (`coverage`'s `Err`, e.g.
/// SemVer target vs. PEP440 spec) aborts the whole cascade with
/// `Err(GraphError::GrammarMismatch)`. Soft instead: `Catalog`/`Opaque`
/// specs never have coverage tested (`coverage` returns `Unknown`
/// unconditionally), so they can't block anything -- `Catalog` gets
/// [`DiagnosticCode::CatalogSpecNotRewritten`], `Opaque` gets no
/// diagnostic. [`DiagnosticCode::RangeNotRoundTrippable`]'s real trigger
/// is a different step: [`rewrite_spec`], when a mechanical range rewrite
/// (not a coverage test) fails on an otherwise-known-coverage spec.
pub fn solve_cascade<D: DependencyResolver>(input: CascadeInput<'_, D>) -> Result<CascadeOutcome, GraphError> {
    let mut out = CascadeOutcome {
        severities: input.seed.clone(),
        reasons: input.reasons.clone(),
        ..Default::default()
    };

    for (id, &sev) in input.seed {
        let t = bump_target(id, sev, &input)?;
        out.targets.insert(id.clone(), t);
    }

    let mut worklist: BTreeSet<PackageId> = out.targets.keys().cloned().collect();
    let mut iterations = 0;
    let bound = convergence_bound(input.graph.packages().count());
    // Edges already run through rewrite_spec, so the floor-raise pass below doesn't re-diagnose them.
    let mut rewrite_attempted: BTreeSet<RewriteKey> = BTreeSet::new();

    let mut changed = true;
    while changed {
        changed = false;

        while let Some(pkg) = worklist.pop_first() {
            iterations += 1;
            if iterations > bound {
                return Err(GraphError::CascadeNotConverged { iterations });
            }

            let new_version = out.targets[&pkg].clone();
            let src_sev = out.severities[&pkg];

            let dependents: Vec<DepEdge> = input.graph.dependents_of(&pkg).cloned().collect();
            for edge in dependents {
                let cov = coverage(&edge.spec, &new_version).map_err(|source| GraphError::GrammarMismatch {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    source,
                })?;

                let d = cascade_action(edge.kind, cov, src_sev, input.cfg);

                if d.unknown_coverage && !matches!(edge.spec, DepSpec::Opaque(_)) {
                    let code = match edge.spec {
                        DepSpec::Catalog(_) => DiagnosticCode::CatalogSpecNotRewritten,
                        _ => DiagnosticCode::RangeNotRoundTrippable,
                    };
                    out.diagnostics.push(Diagnostic {
                        code,
                        severity: DiagnosticSeverity::Warning,
                        message: format!(
                            "spec `{}` for `{}` could not be tested for coverage",
                            edge.spec.render(),
                            edge.to.display_name()
                        ),
                        package: Some(edge.from.clone()),
                        path: Some(edge.from_manifest.clone()),
                        governed_by: Some(ConfigKey::CASCADE_PRESERVE_NPM_RANGES),
                        escalated_by: None,
                    });
                }

                if d.rewrite {
                    let eco = manifest_ecosystem(&edge.from_manifest);
                    match rewrite_spec(&edge.spec, &new_version, eco, input.cfg) {
                        RewriteOutcome::Rewritten(to_spec) => {
                            let key = dependency_rewrite_key(&edge);
                            rewrite_attempted.insert(key.clone());
                            out.rewrites.insert(
                                key.clone(),
                                SpecRewrite {
                                    key,
                                    dependency: edge.to.clone(),
                                    from: edge.spec.clone(),
                                    to: to_spec,
                                },
                            );
                        }
                        RewriteOutcome::LeftAlone(dg) => {
                            rewrite_attempted.insert(dependency_rewrite_key(&edge));
                            out.diagnostics.push(dg);
                        }
                    }
                }

                let cur_sev = out.severities.get(&edge.from).copied().unwrap_or(Severity::None);
                if d.severity > cur_sev {
                    raise(
                        &edge.from,
                        d.severity,
                        &d,
                        &pkg,
                        &edge,
                        &new_version,
                        &mut out,
                        input.groups,
                        &mut worklist,
                        &input,
                    )?;
                }
            }
        }

        // Linked group release severity propagation
        for g in input.groups.linked.values() {
            let max_sev = g.max_severity(&out.severities);
            if max_sev == Severity::None {
                continue;
            }

            // A member absent from `input.base` was removed from the
            // workspace but is still listed in the group config. Skip it
            // rather than let `bump_target` below hit `MissingField` --
            // mirrors the guard `aggregate::union_fixed`/`union_linked`
            // already apply before cascade convergence ever starts.
            let mut live_members = Vec::new();
            for id in g.package_members() {
                if !input.base.contains_key(id) {
                    out.diagnostics.push(Diagnostic {
                        code: DiagnosticCode::UnknownPackage,
                        severity: DiagnosticSeverity::Warning,
                        message: format!(
                            "Linked group `{}` references package `{}` which is not in the \
                             workspace; the stale group member is skipped. Remove it from \
                             callisto.toml to silence this warning.",
                            g.name,
                            id.display_name()
                        ),
                        package: Some(id.clone()),
                        path: None,
                        governed_by: Some(ConfigKey::LINKED_GROUP),
                        escalated_by: None,
                    });
                    continue;
                }
                live_members.push(id.clone());
                let cur_sev = out.severities.get(id).copied().unwrap_or(Severity::None);
                if max_sev > cur_sev {
                    out.severities.insert(id.clone(), max_sev);
                }
            }

            let mut winner: Option<Version> = None;
            for id in &live_members {
                let candidate = bump_target(id, max_sev, &input)?;
                winner = Some(match winner {
                    None => candidate,
                    Some(best) => {
                        let cmp = Version::compare(&candidate, &best).map_err(|_grammar_mismatch| {
                            GraphError::GroupGrammarMismatch {
                                group: g.name.clone(),
                                members: live_members
                                    .iter()
                                    .filter_map(|m| out.targets.get(m).map(|v| (m.clone(), v.clone())))
                                    .collect(),
                            }
                        })?;
                        if cmp.is_gt() {
                            candidate
                        } else {
                            best
                        }
                    }
                });
            }
            let Some(winner) = winner else { continue };

            for id in live_members {
                if out.targets.get(&id) != Some(&winner) {
                    out.targets.insert(id.clone(), winner.clone());
                    out.reasons
                        .insert(id.clone(), BumpReason::LinkedGroupUnion { group: g.name.clone() });
                    worklist.insert(id.clone());
                    changed = true;
                }
            }
        }

        // Fixed group convergence -- mirrors the Linked-group block
        // above but computes each group's shared target via
        // fixed_group_target instead of taking the max of independently
        // bumped per-member candidates.
        for g in input.groups.fixed.values() {
            let max_sev = g.max_severity(&out.severities);
            if max_sev == Severity::None {
                continue;
            }

            // A member absent from `input.base` was removed from the
            // workspace but is still listed in the group config. Skip it
            // rather than let it flow into `out.severities`/`out.targets`,
            // which `plan_version`'s `pkg_map.get(id).copied().unwrap()`
            // (built only from live graph packages) would later panic on --
            // mirrors the guard `aggregate::union_fixed`/`union_linked` and
            // the Linked-group block above already apply.
            let mut live_members = Vec::new();
            for id in g.package_members() {
                if !input.base.contains_key(id) {
                    out.diagnostics.push(Diagnostic {
                        code: DiagnosticCode::UnknownPackage,
                        severity: DiagnosticSeverity::Warning,
                        message: format!(
                            "Fixed group `{}` references package `{}` which is not in the \
                             workspace; the stale group member is skipped. Remove it from \
                             callisto.toml to silence this warning.",
                            g.name,
                            id.display_name()
                        ),
                        package: Some(id.clone()),
                        path: None,
                        governed_by: Some(ConfigKey::FIXED_GROUP),
                        escalated_by: None,
                    });
                    continue;
                }
                live_members.push(id.clone());
                let cur_sev = out.severities.get(id).copied().unwrap_or(Severity::None);
                if max_sev > cur_sev {
                    out.severities.insert(id.clone(), max_sev);
                }
            }

            let winner =
                crate::groups::fixed_group_target(&g.name, &live_members, input.base, max_sev, input.tags, input.pre)?;

            for id in live_members {
                if out.targets.get(&id) != Some(&winner) {
                    out.targets.insert(id.clone(), winner.clone());
                    out.reasons
                        .insert(id.clone(), BumpReason::FixedGroupUnion { group: g.name.clone() });
                    worklist.insert(id.clone());
                    changed = true;
                }
            }
        }
    }

    // A releasing package's floor on a co-released dependency is always raised, even when the old spec still covers it.
    for pkg in out.severities.keys().cloned().collect::<Vec<_>>() {
        let deps: Vec<DepEdge> = input.graph.dependencies_of(&pkg).cloned().collect();
        for edge in deps {
            if edge.kind == DepKind::Dev {
                continue;
            }
            if matches!(
                edge.spec,
                DepSpec::Workspace(_) | DepSpec::Catalog(_) | DepSpec::Opaque(_)
            ) {
                continue;
            }
            let Some(new_version) = out.targets.get(&edge.to).cloned() else {
                continue;
            };
            if matches!(edge.spec, DepSpec::WorkspaceProtocol(_))
                && coverage(&edge.spec, &new_version).map_err(|source| GraphError::GrammarMismatch {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    source,
                })? != Coverage::DoesNotCover
            {
                continue;
            }

            let eco = manifest_ecosystem(&edge.from_manifest);

            let key = dependency_rewrite_key(&edge);
            if rewrite_attempted.contains(&key) {
                continue;
            }

            match rewrite_spec(&edge.spec, &new_version, eco, input.cfg) {
                RewriteOutcome::Rewritten(to_spec) => {
                    out.rewrites.entry(key.clone()).or_insert_with(|| SpecRewrite {
                        key,
                        dependency: edge.to.clone(),
                        from: edge.spec.clone(),
                        to: to_spec,
                    });
                }
                RewriteOutcome::LeftAlone(dg) => {
                    out.diagnostics.push(dg);
                }
            }
        }
    }

    out.iterations = iterations;
    Ok(out)
}

fn bump_target<D: DependencyResolver>(
    id: &PackageId,
    sev: Severity,
    input: &CascadeInput<'_, D>,
) -> Result<Version, GraphError> {
    let base = input.base.get(id).cloned().ok_or_else(|| {
        GraphError::Manifest(callisto_model::ManifestError::MissingField {
            path: PathBuf::from(id.name()),
            field: "version",
        })
    })?;

    // PreMode::Exit falls through to a plain `versioning.bump`, which strips a matching pre-release tag directly.
    crate::groups::versioned_bump(id, &base, sev, input.pre)
}

#[allow(clippy::too_many_arguments)]
fn raise<D: DependencyResolver>(
    pkg: &PackageId,
    sev: Severity,
    decision: &CascadeDecision,
    via: &PackageId,
    edge: &DepEdge,
    dependency_to: &Version,
    out: &mut CascadeOutcome,
    groups: &GroupTable,
    worklist: &mut BTreeSet<PackageId>,
    input: &CascadeInput<'_, D>,
) -> Result<bool, GraphError> {
    let cur_sev = out.severities.get(pkg).copied().unwrap_or(Severity::None);
    if sev <= cur_sev {
        return Ok(false);
    }

    out.severities.insert(pkg.clone(), sev);

    let new_reason = if decision.escalated {
        BumpReason::PeerEscalation {
            via: via.clone(),
            spec: edge.spec.render(),
        }
    } else {
        BumpReason::Cascade {
            via: via.clone(),
            dep_kind: edge.kind,
            spec: edge.spec.render(),
            dependency_to: dependency_to.clone(),
        }
    };
    out.reasons.insert(pkg.clone(), new_reason);

    if let Some(ref gov) = decision.governed_by {
        out.governed_by.insert(pkg.clone(), gov.clone());
    }

    let new_t = bump_target(pkg, sev, input)?;
    out.targets.insert(pkg.clone(), new_t);
    worklist.insert(pkg.clone());

    for sib in groups.fixed_siblings(pkg) {
        let sib_sev = out.severities.get(sib).copied().unwrap_or(Severity::None);
        if sev > sib_sev {
            out.severities.insert(sib.clone(), sev);
            out.reasons.insert(
                sib.clone(),
                BumpReason::FixedGroupUnion {
                    group: groups
                        .fixed_group_of(pkg)
                        .expect("a fixed-group sibling implies the package has a fixed group")
                        .name
                        .clone(),
                },
            );
            let sib_t = bump_target(sib, sev, input)?;
            out.targets.insert(sib.clone(), sib_t);
            worklist.insert(sib.clone());
        }
    }

    Ok(true)
}

pub(crate) fn convergence_bound(package_count: usize) -> usize {
    4 * package_count + 1
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DepWriteTarget {
    Manifest(PathBuf),
    CargoWorkspaceDependency { root_manifest: PathBuf },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RewriteKey {
    pub target: DepWriteTarget,
    pub name: String,
    pub kind: Option<DepKind>,
    pub locator: DepLocator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecRewrite {
    pub key: RewriteKey,
    pub dependency: PackageId,
    pub from: DepSpec,
    pub to: DepSpec,
}

pub enum RewriteOutcome {
    Rewritten(DepSpec),
    LeftAlone(Diagnostic),
}

pub fn rewrite_spec(original: &DepSpec, new: &Version, eco: Ecosystem, cfg: &CascadeConfig) -> RewriteOutcome {
    if !cfg.preserve_npm_ranges && eco == Ecosystem::Npm && !matches!(original, DepSpec::WorkspaceProtocol(_)) {
        return RewriteOutcome::Rewritten(DepSpec::Exact(new.clone()));
    }

    if let Some(rewritten) = callisto_manifests::round_trip(eco, original, new) {
        RewriteOutcome::Rewritten(rewritten)
    } else {
        RewriteOutcome::LeftAlone(Diagnostic {
            code: DiagnosticCode::RangeNotRoundTrippable,
            severity: DiagnosticSeverity::Warning,
            message: format!(
                "spec `{}` could not be round-tripped toward version `{}`",
                original.render(),
                new.render()
            ),
            package: None,
            path: None,
            governed_by: Some(ConfigKey::CASCADE_PRESERVE_NPM_RANGES),
            escalated_by: None,
        })
    }
}

#[cfg(test)]
mod tests;
