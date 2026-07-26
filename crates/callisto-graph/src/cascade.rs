use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use callisto_format::Versioning;
use callisto_model::{
    BumpReason, ConfigKey, Coverage, DepEdge, DepKind, DepSpec, Diagnostic, DiagnosticCode,
    DiagnosticSeverity, Ecosystem, GrammarMismatch, PackageId, Severity, Version,
};

use crate::config::GroupTable;
use crate::config::{CascadeConfig, CascadeMode};
use crate::error::GraphError;
use crate::resolver::DependencyResolver;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CascadeDecision {
    pub severity: Severity,
    pub rewrite: bool,
    pub governed_by: Option<ConfigKey>,
    pub escalated: bool,
    pub unknown_coverage: bool,
}

pub fn cascade_action(
    kind: DepKind,
    coverage: Coverage,
    source: Severity,
    cfg: &CascadeConfig,
) -> CascadeDecision {
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
            (
                Severity::Major,
                Some(ConfigKey::CASCADE_PEER_ESCALATION),
                true,
            )
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
        (CascadeMode::Always, Covers | Unknown, s) if s != Severity::None => {
            Some(ConfigKey::CASCADE_MODE)
        }
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
        DepSpec::Catalog(_) | DepSpec::Opaque(_) => Ok(Coverage::Unknown),
    }
}

pub(crate) fn caret_covers(cur: &Version, new: &Version) -> Result<bool, GrammarMismatch> {
    let cmp = Version::compare(new, cur)?;
    if cmp.is_lt() {
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
    pub pre: Option<&'a callisto_format::PreState>,
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

pub fn run_cascade<D: DependencyResolver>(
    input: CascadeInput<'_, D>,
) -> Result<CascadeOutcome, GraphError> {
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

    while let Some(pkg) = worklist.pop_first() {
        iterations += 1;
        if iterations > bound {
            return Err(GraphError::CascadeNotConverged { iterations });
        }

        let new_version = out.targets[&pkg].clone();
        let src_sev = out.severities[&pkg];

        let dependents: Vec<DepEdge> = input.graph.dependents_of(&pkg).cloned().collect();
        for edge in dependents {
            let cov = coverage(&edge.spec, &new_version).map_err(|source| {
                GraphError::GrammarMismatch {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    source,
                }
            })?;

            let d = cascade_action(edge.kind, cov, src_sev, input.cfg);

            if d.unknown_coverage {
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
                let eco = edge.from.ecosystem().unwrap_or_else(|| {
                    if edge.from_manifest.to_string_lossy().ends_with("Cargo.toml") {
                        Ecosystem::Cargo
                    } else {
                        Ecosystem::Npm
                    }
                });
                match rewrite_spec(&edge.spec, &new_version, eco, input.cfg) {
                    RewriteOutcome::Rewritten(to_spec) => {
                        let key = RewriteKey {
                            target: if edge.inherited {
                                DepWriteTarget::CargoWorkspaceDependency {
                                    root_manifest: edge.from_manifest.clone(),
                                }
                            } else {
                                DepWriteTarget::Manifest(edge.from_manifest.clone())
                            },
                            name: edge.to.name().to_string(),
                            kind: if edge.inherited {
                                None
                            } else {
                                Some(edge.kind)
                            },
                        };
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
                        out.diagnostics.push(dg);
                    }
                }
            }

            let cur_sev = out
                .severities
                .get(&edge.from)
                .copied()
                .unwrap_or(Severity::None);
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

    out.iterations = iterations;
    Ok(out)
}

fn bump_target<D: DependencyResolver>(
    id: &PackageId,
    sev: Severity,
    input: &CascadeInput<'_, D>,
) -> Result<Version, GraphError> {
    let base = input
        .base
        .get(id)
        .cloned()
        .unwrap_or_else(|| Version::semver(1, 0, 0));
    let versioning = callisto_format::SemVerVersioning;

    if let Some(pre) = input.pre {
        versioning
            .bump_prerelease(&base, sev, &pre.tag, &base)
            .map_err(GraphError::Bump)
    } else {
        versioning.bump(&base, sev).map_err(GraphError::Bump)
    }
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
    out.reasons.entry(pkg.clone()).or_insert_with(|| {
        if decision.escalated {
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
        }
    });

    if let Some(gov) = &decision.governed_by {
        out.governed_by.entry(pkg.clone()).or_insert(gov.clone());
    }

    let new_t = bump_target(pkg, sev, input)?;
    out.targets.insert(pkg.clone(), new_t);
    worklist.insert(pkg.clone());

    for sib in groups.fixed_siblings(pkg) {
        let sib_sev = out.severities.get(sib).copied().unwrap_or(Severity::None);
        if sev > sib_sev {
            out.severities.insert(sib.clone(), sev);
            out.reasons
                .entry(sib.clone())
                .or_insert(BumpReason::FixedGroupUnion {
                    group: groups.fixed_group_of(pkg).unwrap().name.clone(),
                });
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

pub fn rewrite_spec(
    original: &DepSpec,
    new: &Version,
    eco: Ecosystem,
    cfg: &CascadeConfig,
) -> RewriteOutcome {
    if !cfg.preserve_npm_ranges && eco == Ecosystem::Npm {
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
