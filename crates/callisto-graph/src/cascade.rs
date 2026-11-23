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

mod converged;
pub use converged::{ConvergedTarget, ConvergedTargets};

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
    // A caret requirement always matches the version it names, pre-release or not.
    if new == cur {
        return Ok(true);
    }
    // A pre-release is never covered by a caret range (^1.2.3 does not cover 1.9.0-alpha.1).
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
    pub base: &'a BTreeMap<PackageId, Version>,
    pub pre: Option<&'a callisto_model::format::PreState>,
    pub tags: &'a TagIndex,
    pub identity: &'a IdentityIndex,
}

#[derive(Clone, Debug, Default)]
pub struct CascadeOutcome {
    pub severities: BTreeMap<PackageId, Severity>,
    pub targets: ConvergedTargets,
    pub reasons: BTreeMap<PackageId, BumpReason>,
    pub governed_by: BTreeMap<PackageId, ConfigKey>,
    pub rewrites: BTreeMap<RewriteKey, SpecRewrite>,
    /// Per consumer, the bumped dependencies it was updated for; the changelog's "Dependency updates" list.
    pub dependency_updates: BTreeMap<PackageId, BTreeMap<PackageId, DepKind>>,
    pub diagnostics: Vec<Diagnostic>,
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

/// One cascade raise on a consumer, derived from the targets a round settled.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Raise {
    severity: Severity,
    reason: BumpReason,
    governed_by: Option<ConfigKey>,
}

/// The explicit seeds plus one set of cascade raises, with every group aligned.
#[derive(Default)]
struct Round {
    severities: BTreeMap<PackageId, Severity>,
    reasons: BTreeMap<PackageId, BumpReason>,
    governed_by: BTreeMap<PackageId, ConfigKey>,
    targets: BTreeMap<PackageId, Version>,
    diagnostics: Vec<Diagnostic>,
}

/// Releases `input.seed` and every consumer whose range rejects a released package's final target.
///
/// Each round starts again from the explicit seeds (changesets, inference) plus the cascade raises the previous
/// round required, aligns linked and fixed groups, and derives the raises its settled targets require. A raise
/// required only by an intermediate target is dropped once the final target satisfies the consumer's range. The run
/// stops when a round requires exactly the raises it started from and each is reached from an explicit seed through
/// edges active under the settled targets; unreached raises (a group sustaining a raise no seed requires) are dropped
/// and the round settled again. After one round per package, each round also keeps the previous raises, so the raise
/// map only grows (at most `3 * packages` times); dropping unreached raises there leaves the reached packages' targets
/// unchanged, so the next round is a fixed point. The run settles within `4 * packages + 2` rounds; otherwise
/// [`GraphError::CascadeNotConverged`] reports the bug.
///
/// A genuine cross-grammar coverage failure (`coverage`'s `Err`, e.g.
/// SemVer target vs. PEP440 spec) aborts the whole cascade with
/// `Err(GraphError::GrammarMismatch)`. Soft instead: `Catalog`/`Opaque`
/// specs never have coverage tested (`coverage` returns `Unknown`
/// unconditionally), so they can't block anything -- `Catalog` gets
/// [`DiagnosticCode::CatalogSpecNotRewritten`], `Opaque` gets no
/// diagnostic. [`DiagnosticCode::RangeNotRoundTrippable`]'s real trigger
/// is a different step: `rewrite_spec`, when a mechanical range rewrite
/// (not a coverage test) fails on an otherwise-known-coverage spec.
pub fn solve_cascade<D: DependencyResolver>(input: CascadeInput<'_, D>) -> Result<CascadeOutcome, GraphError> {
    let packages = input.graph.packages().count();
    let bound = convergence_bound(packages);
    let mut raises: BTreeMap<PackageId, Raise> = BTreeMap::new();
    let mut rounds = 0;
    let (round, coverage_diagnostics) = loop {
        rounds += 1;
        if rounds > bound {
            return Err(GraphError::CascadeNotConverged { iterations: rounds });
        }
        let round = settle_round(&input, &raises)?;
        let (mut required, active, diagnostics) = required_raises(&input, &round)?;
        if rounds > packages {
            for (id, kept) in &raises {
                if required.get(id).is_none_or(|r| r.severity <= kept.severity) {
                    required.insert(id.clone(), kept.clone());
                }
            }
        }
        if required == raises {
            let reached = reached_from_seeds(&input, &round, &active);
            if raises.keys().all(|id| reached.contains(id)) {
                break (round, diagnostics);
            }
            required.retain(|id, _| reached.contains(id));
        }
        raises = required;
    };

    let mut out = CascadeOutcome {
        severities: round.severities,
        targets: ConvergedTargets::settle(round.targets),
        reasons: round.reasons,
        governed_by: round.governed_by,
        diagnostics: round.diagnostics,
        ..Default::default()
    };
    out.diagnostics.extend(coverage_diagnostics);
    // Edges already run through rewrite_spec, so the floor-raise pass below doesn't re-diagnose them.
    let mut rewrite_attempted: BTreeSet<RewriteKey> = BTreeSet::new();

    // A raise kept from an earlier round can name an intermediate version of `via`, so name the one that is released.
    for reason in out.reasons.values_mut() {
        if let BumpReason::Cascade { via, dependency_to, .. } = reason
            && let Some(settled) = out.targets.get(via)
        {
            *dependency_to = settled.version().clone();
        }
    }

    // Rewrites and rejections are derived from the converged targets only.
    for (pkg, new_version) in out.targets.iter() {
        let src_sev = out.severities[pkg];
        for edge in input.graph.dependents_of(pkg) {
            let cov = coverage(&edge.spec, new_version.version()).map_err(|source| GraphError::GrammarMismatch {
                from: edge.from.clone(),
                to: edge.to.clone(),
                source,
            })?;
            let d = cascade_action(edge.kind, cov, src_sev, input.cfg);

            if d.rewrite || d.severity > Severity::None {
                out.dependency_updates
                    .entry(edge.from.clone())
                    .or_default()
                    .insert(edge.to.clone(), edge.kind);
            }

            if d.rewrite {
                match rewrite_edge(edge, new_version, input.cfg)? {
                    RewriteOutcome::Rewritten(to_spec) => {
                        let key = dependency_rewrite_key(edge);
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
                        rewrite_attempted.insert(dependency_rewrite_key(edge));
                        out.diagnostics.push(dg);
                    }
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
            let Some(new_version) = out.targets.get(&edge.to) else {
                continue;
            };
            if matches!(edge.spec, DepSpec::WorkspaceProtocol(_))
                && coverage(&edge.spec, new_version.version()).map_err(|source| GraphError::GrammarMismatch {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    source,
                })? != Coverage::DoesNotCover
            {
                continue;
            }

            let key = dependency_rewrite_key(&edge);
            if rewrite_attempted.contains(&key) {
                continue;
            }

            // A floor that cannot be raised (`*`, a bare Python requirement, `||` sets) is harmless while it covers.
            if matches!(coverage(&edge.spec, new_version.version()), Ok(Coverage::Covers))
                && matches!(
                    rewrite_spec(
                        &edge.spec,
                        new_version,
                        manifest_ecosystem(&edge.from_manifest),
                        input.cfg
                    ),
                    RewriteOutcome::LeftAlone(_)
                )
            {
                continue;
            }

            match rewrite_edge(&edge, new_version, input.cfg)? {
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

    Ok(out)
}

/// Severities, reasons and group-aligned targets for the explicit seeds plus `raises`.
fn settle_round<D: DependencyResolver>(
    input: &CascadeInput<'_, D>,
    raises: &BTreeMap<PackageId, Raise>,
) -> Result<Round, GraphError> {
    let mut round = Round {
        severities: input.seed.clone(),
        reasons: input.reasons.clone(),
        ..Default::default()
    };
    for (id, raise) in raises {
        if raise.severity > round.severities.get(id).copied().unwrap_or(Severity::None) {
            round.severities.insert(id.clone(), raise.severity);
            round.reasons.insert(id.clone(), raise.reason.clone());
            if let Some(gov) = &raise.governed_by {
                round.governed_by.insert(id.clone(), gov.clone());
            }
        }
    }
    for (id, &sev) in &round.severities {
        round.targets.insert(id.clone(), bump_target(id, sev, input)?);
    }

    // Linked group release severity propagation
    for g in input.groups.linked.values() {
        let max_sev = g.max_severity(&round.severities);
        if max_sev == Severity::None {
            continue;
        }

        // Skip members absent from `input.base`: `bump_target` would hit `MissingField`.
        let mut live_members = Vec::new();
        for id in g.package_members() {
            if !input.base.contains_key(id) {
                round.diagnostics.push(Diagnostic {
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
            let cur_sev = round.severities.get(id).copied().unwrap_or(Severity::None);
            if max_sev > cur_sev {
                round.severities.insert(id.clone(), max_sev);
            }
        }

        let mut winner: Option<Version> = None;
        for id in &live_members {
            let candidate = bump_target(id, max_sev, input)?;
            winner = Some(match winner {
                None => candidate,
                Some(best) => {
                    let cmp = Version::compare(&candidate, &best).map_err(|_grammar_mismatch| {
                        GraphError::GroupGrammarMismatch {
                            group: g.name.clone(),
                            members: live_members
                                .iter()
                                .filter_map(|m| round.targets.get(m).map(|v| (m.clone(), v.clone())))
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
            if round.targets.get(&id) != Some(&winner) {
                round.targets.insert(id.clone(), winner.clone());
                round
                    .reasons
                    .insert(id.clone(), BumpReason::LinkedGroupUnion { group: g.name.clone() });
            }
        }
    }

    // Fixed groups converge on `fixed_group_target`, not the max of per-member candidates.
    for g in input.groups.fixed.values() {
        let max_sev = g.max_severity(&round.severities);
        if max_sev == Severity::None {
            continue;
        }

        // Skip members absent from `input.base`: they would reach `round.severities` and panic in `plan_version`.
        let mut live_members = Vec::new();
        for id in g.package_members() {
            if !input.base.contains_key(id) {
                round.diagnostics.push(Diagnostic {
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
            let cur_sev = round.severities.get(id).copied().unwrap_or(Severity::None);
            if max_sev > cur_sev {
                round.severities.insert(id.clone(), max_sev);
            }
        }

        let winner =
            crate::groups::fixed_group_target(&g.name, &live_members, input.base, max_sev, input.tags, input.pre)?;

        for id in live_members {
            if round.targets.get(&id) != Some(&winner) {
                round.targets.insert(id.clone(), winner.clone());
                round
                    .reasons
                    .insert(id.clone(), BumpReason::FixedGroupUnion { group: g.name.clone() });
            }
        }
    }

    Ok(round)
}

type RaiseMap = BTreeMap<PackageId, Raise>;

/// Per released package, the dependents its settled target raises.
type ActiveEdges = BTreeMap<PackageId, BTreeSet<PackageId>>;

/// The raise each consumer needs for the targets `round` settled, the edges that raise one, and a warning per range
/// that cannot be tested.
fn required_raises<D: DependencyResolver>(
    input: &CascadeInput<'_, D>,
    round: &Round,
) -> Result<(RaiseMap, ActiveEdges, Vec<Diagnostic>), GraphError> {
    let mut required: BTreeMap<PackageId, Raise> = BTreeMap::new();
    let mut active = ActiveEdges::new();
    let mut diagnostics = Vec::new();
    for (pkg, new_version) in &round.targets {
        let src_sev = round.severities[pkg];
        for edge in input.graph.dependents_of(pkg) {
            let cov = coverage(&edge.spec, new_version).map_err(|source| GraphError::GrammarMismatch {
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
                diagnostics.push(Diagnostic {
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

            if d.severity > Severity::None {
                active.entry(pkg.clone()).or_default().insert(edge.from.clone());
            }
            if d.severity > required.get(&edge.from).map_or(Severity::None, |r| r.severity) {
                let reason = if d.escalated {
                    BumpReason::PeerEscalation {
                        via: pkg.clone(),
                        spec: edge.spec.render(),
                    }
                } else {
                    BumpReason::Cascade {
                        via: pkg.clone(),
                        dep_kind: edge.kind,
                        spec: edge.spec.render(),
                        dependency_to: new_version.clone(),
                    }
                };
                required.insert(
                    edge.from.clone(),
                    Raise {
                        severity: d.severity,
                        reason,
                        governed_by: d.governed_by,
                    },
                );
            }
        }
    }
    Ok((required, active, diagnostics))
}

/// Released packages reached from an explicit seed through `active` edges and the groups of reached members.
fn reached_from_seeds<D: DependencyResolver>(
    input: &CascadeInput<'_, D>,
    round: &Round,
    active: &ActiveEdges,
) -> BTreeSet<PackageId> {
    let mut reached = BTreeSet::new();
    let mut queue: Vec<PackageId> = input.seed.keys().cloned().collect();
    while let Some(id) = queue.pop() {
        if !round.targets.contains_key(&id) || !reached.insert(id.clone()) {
            continue;
        }
        let fixed = input.groups.fixed_group_of(&id);
        let linked = input
            .groups
            .linked_of
            .get(&id)
            .and_then(|name| input.groups.linked.get(name));
        queue.extend(
            [fixed, linked]
                .into_iter()
                .flatten()
                .flat_map(|g| g.package_members().cloned()),
        );
        queue.extend(active.get(&id).into_iter().flatten().cloned());
    }
    reached
}

fn bump_target<D: DependencyResolver>(
    id: &PackageId,
    sev: Severity,
    input: &CascadeInput<'_, D>,
) -> Result<Version, GraphError> {
    // A fixed-group member's own bump is never visible: its target is always the group's aligned version.
    if let Some(group) = input.groups.fixed_group_of(id) {
        let live: Vec<PackageId> = group
            .package_members()
            .filter(|member| input.base.contains_key(*member))
            .cloned()
            .collect();
        if !live.is_empty() {
            return crate::groups::fixed_group_target(&group.name, &live, input.base, sev, input.tags, input.pre);
        }
    }

    let base = input.base.get(id).cloned().ok_or_else(|| {
        GraphError::Manifest(callisto_model::ManifestError::MissingField {
            path: PathBuf::from(id.name()),
            field: "version",
        })
    })?;

    // PreMode::Exit falls through to a plain `versioning.bump`, which strips a matching pre-release tag directly.
    crate::groups::versioned_bump(id, &base, sev, input.pre)
}

/// Each dependency range that excludes its package's snapshot version, rewritten; a snapshot has no cascade or groups.
pub(crate) fn snapshot_rewrites<D: DependencyResolver>(
    graph: &D,
    versions: BTreeMap<PackageId, Version>,
    cfg: &CascadeConfig,
) -> Result<(Vec<SpecRewrite>, Vec<Diagnostic>), GraphError> {
    let settled = ConvergedTargets::settle(versions);
    let mut rewrites: BTreeMap<RewriteKey, SpecRewrite> = BTreeMap::new();
    let mut diagnostics = Vec::new();

    for pkg in graph.packages() {
        for edge in graph.dependencies_of(&pkg.id) {
            if matches!(edge.spec, DepSpec::Workspace(_) | DepSpec::Catalog(_)) {
                if matches!(edge.spec, DepSpec::Catalog(_)) {
                    diagnostics.push(Diagnostic {
                        code: DiagnosticCode::CatalogSpecNotRewritten,
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
                continue;
            }
            if matches!(edge.spec, DepSpec::Opaque(_)) {
                continue;
            }
            let Some(snap_to) = settled.get(&edge.to) else {
                continue;
            };
            // A spec in another grammar than the snapshot version surfaces as a GrammarMismatch, not silently.
            let covers = coverage(&edge.spec, snap_to.version()).map_err(|source| GraphError::GrammarMismatch {
                from: edge.from.clone(),
                to: edge.to.clone(),
                source,
            })?;
            if matches!(covers, Coverage::Covers) {
                continue;
            }

            match rewrite_edge(edge, snap_to, cfg)? {
                RewriteOutcome::Rewritten(to_spec) => {
                    let key = dependency_rewrite_key(edge);
                    rewrites.entry(key.clone()).or_insert_with(|| SpecRewrite {
                        key,
                        dependency: edge.to.clone(),
                        from: edge.spec.clone(),
                        to: to_spec,
                    });
                }
                RewriteOutcome::LeftAlone(dg) => {
                    diagnostics.push(dg);
                }
            }
        }
    }
    Ok((rewrites.into_values().collect(), diagnostics))
}

pub(crate) fn convergence_bound(package_count: usize) -> usize {
    4 * package_count + 2
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

/// Rewrites one dependency edge toward `new`. A range a consumer's published manifest would keep excluding `new`
/// stops the run; only a dev dependency, which consumers never install, is left alone with a warning.
pub(crate) fn rewrite_edge(
    edge: &DepEdge,
    new: ConvergedTarget<'_>,
    cfg: &CascadeConfig,
) -> Result<RewriteOutcome, GraphError> {
    let eco = manifest_ecosystem(&edge.from_manifest);
    match rewrite_spec(&edge.spec, new, eco, cfg) {
        RewriteOutcome::LeftAlone(_) if edge.kind != DepKind::Dev => Err(GraphError::UnrewritableRange {
            dependent: edge.from.display_name(),
            dependency: edge.to.display_name(),
            spec: edge.spec.render(),
            version: new.version().render().to_string(),
            manifest: edge.from_manifest.display().to_string(),
        }),
        outcome => Ok(outcome),
    }
}

pub(crate) fn rewrite_spec(
    original: &DepSpec,
    new: ConvergedTarget<'_>,
    eco: Ecosystem,
    cfg: &CascadeConfig,
) -> RewriteOutcome {
    let new = new.version();
    if !cfg.preserve_npm_ranges && eco == Ecosystem::Npm && !matches!(original, DepSpec::WorkspaceProtocol(_)) {
        return RewriteOutcome::Rewritten(DepSpec::Exact(new.clone()));
    }

    // A rewrite that still excludes `new` is no rewrite: the dependent would publish a range its dependency cannot meet.
    match callisto_manifests::round_trip(eco, original, new)
        .filter(|rewritten| matches!(coverage(rewritten, new), Ok(Coverage::Covers | Coverage::Unknown)))
    {
        Some(rewritten) => RewriteOutcome::Rewritten(rewritten),
        _ => RewriteOutcome::LeftAlone(Diagnostic {
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
        }),
    }
}

#[cfg(test)]
mod tests;
