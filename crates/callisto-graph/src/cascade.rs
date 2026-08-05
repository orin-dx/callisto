use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

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
    _source: Severity,
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
        (Peer, DoesNotCover) if cfg.peer_escalation && matches!(coverage, DoesNotCover) => (
            Severity::Major,
            Some(ConfigKey::CASCADE_PEER_ESCALATION),
            true,
        ),
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

/// Trait for dependency graph cascade solving.
pub trait CascadeSolver<D: DependencyResolver> {
    fn solve_cascade(&self, input: CascadeInput<'_, D>) -> Result<CascadeOutcome, GraphError>;
}

pub fn run_cascade<D: DependencyResolver>(
    input: CascadeInput<'_, D>,
) -> Result<CascadeOutcome, GraphError> {
    solve_cascade(input)
}

pub fn solve_cascade<D: DependencyResolver>(
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
                let cov = coverage(&edge.spec, &new_version).map_err(|source| {
                    GraphError::GrammarMismatch {
                        from: edge.from.clone(),
                        to: edge.to.clone(),
                        source,
                    }
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

        // Spec §G.6.7: Linked group release severity propagation
        for g in input.groups.linked.values() {
            let member_ids: Vec<PackageId> = g
                .members(crate::config::GroupMemberKind::Package)
                .filter_map(|m| match m {
                    crate::config::GroupMember::Package(ref id) => Some(id.clone()),
                    _ => None,
                })
                .collect();

            let mut max_sev = Severity::None;
            for id in &member_ids {
                if let Some(&sev) = out.severities.get(id) {
                    max_sev = max_sev.max(sev);
                }
            }

            if max_sev > Severity::None {
                for id in &member_ids {
                    let cur_sev = out.severities.get(id).copied().unwrap_or(Severity::None);
                    if max_sev > cur_sev {
                        out.severities.insert(id.clone(), max_sev);
                    }
                }

                let mut winner: Option<Version> = None;
                for id in &member_ids {
                    let candidate = bump_target(id, max_sev, &input)?;
                    winner = Some(match winner {
                        None => candidate,
                        Some(best) => {
                            let cmp = Version::compare(&candidate, &best).map_err(
                                |_grammar_mismatch| GraphError::GroupGrammarMismatch {
                                    group: g.name.clone(),
                                    members: member_ids
                                        .iter()
                                        .filter_map(|m| {
                                            out.targets.get(m).map(|v| (m.clone(), v.clone()))
                                        })
                                        .collect(),
                                },
                            )?;
                            if cmp.is_gt() {
                                candidate
                            } else {
                                best
                            }
                        }
                    });
                }
                let winner = winner.expect("linked group has at least one member");

                for id in member_ids {
                    if out.targets.get(&id) != Some(&winner) {
                        out.targets.insert(id.clone(), winner.clone());
                        out.reasons.insert(
                            id.clone(),
                            BumpReason::LinkedGroupUnion {
                                group: g.name.clone(),
                            },
                        );
                        worklist.insert(id.clone());
                        changed = true;
                    }
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
    let versioning: &dyn callisto_format::Versioning = match base.grammar() {
        callisto_model::VersionGrammar::Pep440 => &callisto_format::Pep440Versioning,
        _ => &callisto_format::SemVerVersioning,
    };

    if let Some(pre) = input.pre {
        if pre.mode == callisto_format::PreMode::Pre {
            let pinned_base = pre.initial_versions.get(id.name()).unwrap_or(&base);
            versioning
                .bump_prerelease(pinned_base, sev, &pre.tag, &base)
                .map_err(GraphError::Bump)
        } else {
            // PreMode::Exit: finalize the current pre-release to a stable version.
            // Bumping the on-disk pre-release version (e.g. "1.0.0-alpha.2") with
            // any severity that matches the pre-release target strips the tag and
            // produces the stable version (e.g. "1.0.0").
            versioning.bump(&base, sev).map_err(GraphError::Bump)
        }
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
                    group: groups.fixed_group_of(pkg).unwrap().name.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::{GroupKind, GroupName, Package, ReleaseTrigger};

    use crate::config::{CascadeBumpSeverity, GroupDef, GroupMember};

    struct TwoPackageGraph {
        packages: Vec<Package>,
    }

    impl DependencyResolver for TwoPackageGraph {
        fn packages(&self) -> impl Iterator<Item = &Package> {
            self.packages.iter()
        }

        fn dependencies_of(&self, _id: &PackageId) -> impl Iterator<Item = &DepEdge> {
            std::iter::empty()
        }

        fn dependents_of(&self, _id: &PackageId) -> impl Iterator<Item = &DepEdge> {
            std::iter::empty()
        }
    }

    fn bare_package(id: &PackageId) -> Package {
        Package {
            id: id.clone(),
            manifests: Vec::new(),
            changelog: None,
            release_trigger: ReleaseTrigger::Changeset,
            publish_to: Vec::new(),
            tag_template: None,
        }
    }

    /// Spec §G.6.7: a severity bump landing on a single member of a linked
    /// group (e.g. via a changeset naming only that package) must propagate
    /// to every other member of the group, and every member must converge
    /// on a stable target version.
    #[test]
    fn test_linked_group_propagates_severity_to_unseeded_member_and_converges() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let pkg_b = PackageId::parse("pkg-b").unwrap();

        let graph = TwoPackageGraph {
            packages: vec![bare_package(&pkg_a), bare_package(&pkg_b)],
        };

        let mut base = BTreeMap::new();
        base.insert(pkg_a.clone(), Version::semver(1, 0, 0));
        base.insert(pkg_b.clone(), Version::semver(1, 0, 0));

        // Only pkg_b receives a severity via a changeset; pkg_a starts
        // completely unseeded.
        let mut seed = BTreeMap::new();
        seed.insert(pkg_b.clone(), Severity::Major);

        let mut groups = GroupTable::default();
        let group_def = GroupDef {
            name: GroupName("linked-pair".to_string()),
            kind: GroupKind::Linked,
            members: vec![
                GroupMember::Package(pkg_a.clone()),
                GroupMember::Package(pkg_b.clone()),
            ],
        };
        groups.linked.insert(group_def.name.clone(), group_def);

        let cfg = CascadeConfig {
            mode: CascadeMode::OutOfRange,
            bump_severity: CascadeBumpSeverity::Patch,
            peer_escalation: true,
            preserve_npm_ranges: false,
        };

        let reasons = BTreeMap::new();
        let named_by = BTreeMap::new();

        let input = CascadeInput {
            graph: &graph,
            groups: &groups,
            cfg: &cfg,
            seed: &seed,
            reasons: &reasons,
            named_by: &named_by,
            base: &base,
            pre: None,
        };

        let outcome = run_cascade(input).unwrap();

        // pkg_a was never seeded directly; it must pick up pkg_b's severity
        // purely through linked-group union.
        assert_eq!(outcome.severities.get(&pkg_a), Some(&Severity::Major));
        assert_eq!(outcome.severities.get(&pkg_b), Some(&Severity::Major));

        let target_a = outcome.targets.get(&pkg_a).unwrap();
        let target_b = outcome.targets.get(&pkg_b).unwrap();

        // Both members share the same base version, so convergence must
        // land them on the exact same target version.
        assert_eq!(target_a, target_b);
        assert_eq!(target_a.render(), "2.0.0");

        assert_eq!(
            outcome.reasons.get(&pkg_a),
            Some(&BumpReason::LinkedGroupUnion {
                group: GroupName("linked-pair".to_string()),
            })
        );
    }

    /// Spec §G.6.7: linked group members with *different* base versions must
    /// converge on the SAME winning target version (the max of each member's
    /// individually-computed candidate at the converged severity), not just
    /// the same severity.
    #[test]
    fn test_linked_group_converges_target_version_across_divergent_bases() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let pkg_b = PackageId::parse("pkg-b").unwrap();

        let graph = TwoPackageGraph {
            packages: vec![bare_package(&pkg_a), bare_package(&pkg_b)],
        };

        let mut base = BTreeMap::new();
        base.insert(
            pkg_a.clone(),
            Version::parse("1.4.0", callisto_model::VersionGrammar::SemVer).unwrap(),
        );
        base.insert(
            pkg_b.clone(),
            Version::parse("2.7.3", callisto_model::VersionGrammar::SemVer).unwrap(),
        );

        let mut seed = BTreeMap::new();
        seed.insert(pkg_a.clone(), Severity::Minor);

        let mut groups = GroupTable::default();
        let group_def = GroupDef {
            name: GroupName("linked-pair".to_string()),
            kind: GroupKind::Linked,
            members: vec![
                GroupMember::Package(pkg_a.clone()),
                GroupMember::Package(pkg_b.clone()),
            ],
        };
        groups.linked.insert(group_def.name.clone(), group_def);

        let cfg = CascadeConfig {
            mode: CascadeMode::OutOfRange,
            bump_severity: CascadeBumpSeverity::Patch,
            peer_escalation: true,
            preserve_npm_ranges: false,
        };

        let reasons = BTreeMap::new();
        let named_by = BTreeMap::new();

        let input = CascadeInput {
            graph: &graph,
            groups: &groups,
            cfg: &cfg,
            seed: &seed,
            reasons: &reasons,
            named_by: &named_by,
            base: &base,
            pre: None,
        };

        let outcome = run_cascade(input).unwrap();

        let target_a = outcome.targets.get(&pkg_a).unwrap();
        let target_b = outcome.targets.get(&pkg_b).unwrap();

        // pkg_a at minor from 1.4.0 -> 1.5.0; pkg_b at minor from 2.7.3 -> 2.8.0.
        // The winner (max by Version::compare) is 2.8.0 and both must converge on it.
        assert_eq!(target_a, target_b);
        assert_eq!(target_a.render(), "2.8.0");
    }

    /// When `pre.mode == PreMode::Exit`, `bump_target` must finalize the current
    /// pre-release to a stable version instead of producing another pre-release
    /// iteration. The test uses `base = "1.0.0-alpha.2"` (current on-disk prerelease)
    /// and asserts the result is the stable `"1.0.0"`, not another `-alpha.N`.
    ///
    /// Calling `versioning.bump` on a pre-release version strips the pre-release
    /// tag and finalizes in-place when the underlying release segment already
    /// matches the requested bump (patch=0, minor=0 for a minor bump).
    #[test]
    fn test_bump_target_exit_mode_finalizes_to_stable() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();

        let graph = TwoPackageGraph {
            packages: vec![bare_package(&pkg_a)],
        };

        // On-disk version is at a major pre-release (1.0.0-alpha.2).
        let mut base = BTreeMap::new();
        base.insert(
            pkg_a.clone(),
            Version::parse("1.0.0-alpha.2", callisto_model::VersionGrammar::SemVer).unwrap(),
        );

        let mut initial_versions = indexmap::IndexMap::new();
        initial_versions.insert(
            "pkg-a".to_string(),
            Version::parse("0.9.0", callisto_model::VersionGrammar::SemVer).unwrap(),
        );

        // PreMode::Exit: the release cycle is being finalized.
        let pre = callisto_format::PreState {
            mode: callisto_format::PreMode::Exit,
            tag: "alpha".to_string(),
            initial_versions,
            changesets: Vec::new(),
        };

        let groups = GroupTable::default();
        let cfg = CascadeConfig {
            mode: CascadeMode::OutOfRange,
            bump_severity: CascadeBumpSeverity::Patch,
            peer_escalation: true,
            preserve_npm_ranges: false,
        };

        let seed = BTreeMap::new();
        let reasons = BTreeMap::new();
        let named_by = BTreeMap::new();

        let input = CascadeInput {
            graph: &graph,
            groups: &groups,
            cfg: &cfg,
            seed: &seed,
            reasons: &reasons,
            named_by: &named_by,
            base: &base,
            pre: Some(&pre),
        };

        let target = bump_target(&pkg_a, Severity::Minor, &input).unwrap();

        assert!(
            !target.is_prerelease(),
            "PreMode::Exit must produce a stable version, not a pre-release; got {}",
            target.render()
        );
        assert_eq!(
            target.render(),
            "1.0.0",
            "bump_target with Exit mode, base=1.0.0-alpha.2, sev=Minor must finalize to 1.0.0"
        );
    }

    /// A pre-release cycle must bump from the PINNED baseline captured in
    /// `PreState.initial_versions` at `pre enter` time, not from whatever the
    /// latest on-disk prerelease happens to be. Otherwise the release segment
    /// re-derives from a moving target instead of the pinned one.
    #[test]
    fn test_bump_target_uses_pinned_pre_baseline_not_current_prerelease() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();

        let graph = TwoPackageGraph {
            packages: vec![bare_package(&pkg_a)],
        };

        // On-disk is already one prerelease bump into a MAJOR cycle
        // (2.0.0-next.0), while the pinned baseline from `pre enter` is
        // still 1.0.0. Requesting a Minor bump now must be computed from
        // the pinned baseline (-> release 1.1.0), not by re-deriving from
        // the already-major-bumped on-disk value (which would incorrectly
        // treat 2.0.0-next.0 as still-in-progress and yield 2.0.0-next.1).
        let mut base = BTreeMap::new();
        base.insert(
            pkg_a.clone(),
            Version::parse("2.0.0-next.0", callisto_model::VersionGrammar::SemVer).unwrap(),
        );

        let mut initial_versions = indexmap::IndexMap::new();
        initial_versions.insert(
            "pkg-a".to_string(),
            Version::parse("1.0.0", callisto_model::VersionGrammar::SemVer).unwrap(),
        );
        let pre = callisto_format::PreState {
            mode: callisto_format::PreMode::Pre,
            tag: "next".to_string(),
            initial_versions,
            changesets: Vec::new(),
        };

        let groups = GroupTable::default();
        let cfg = CascadeConfig {
            mode: CascadeMode::OutOfRange,
            bump_severity: CascadeBumpSeverity::Patch,
            peer_escalation: true,
            preserve_npm_ranges: false,
        };

        let seed = BTreeMap::new();
        let reasons = BTreeMap::new();
        let named_by = BTreeMap::new();

        let input = CascadeInput {
            graph: &graph,
            groups: &groups,
            cfg: &cfg,
            seed: &seed,
            reasons: &reasons,
            named_by: &named_by,
            base: &base,
            pre: Some(&pre),
        };

        let target = bump_target(&pkg_a, Severity::Minor, &input).unwrap();

        assert_eq!(target.render(), "1.1.0-next.0");
    }
}
