use std::collections::{BTreeMap, BTreeSet, HashSet};

use callisto_model::{DepKind, PackageId, ReleasePackageId};
use michi_graph::{Graph, GraphBuilder, Order, UnknownTargets, Violation, Violations};

use crate::error::GraphError;

/// Topologically sorts `subset` by the edges `outgoing_edges` reports, counting only edges
/// whose [`DepKind`] appears in `ordering_kinds` as ordering constraints. Different callers
/// need different kinds to count: cascade/version-bump propagation only cares about
/// `Runtime`/`Build`/`Optional` (a `Dev`-only change correctly shouldn't force a consumer's
/// version to bump), while publish ordering ([`PublishWaits`]) also needs
/// `Dev`, since `cargo publish`'s own local verification build needs every declared
/// dependency — dev included — resolvable. This function has no opinion on which kinds
/// matter; the caller states it explicitly via `ordering_kinds`.
pub fn toposort_impl<F>(
    subset: &HashSet<PackageId>,
    all_packages: &[PackageId],
    ordering_kinds: &[DepKind],
    outgoing_edges: F,
) -> Result<Vec<PackageId>, GraphError>
where
    F: Fn(&PackageId) -> Vec<(PackageId, DepKind)>,
{
    let members: BTreeSet<PackageId> = subset.iter().cloned().collect();
    // Build a HashSet once so validation is O(N) rather than O(N²) slice scans.
    let all_set: std::collections::HashSet<&PackageId> = all_packages.iter().collect();
    for id in &members {
        if !all_set.contains(id) {
            return Err(GraphError::UnknownPackage {
                id: id.clone(),
                hint: String::new(),
            });
        }
    }

    let dag = member_graph(&members, &outgoing_edges)
        .validate_filtered(|edge| ordering_kinds.contains(edge.kind), Order::Key)
        .map_err(|violations| GraphError::Cycle {
            cycle: cycle_of(&violations),
        })?;
    Ok(dag.keys_in_order().cloned().collect())
}

/// The members as a graph, in key order, with every edge between two members. Which edges count is chosen when the
/// graph is validated, so one graph serves cascade ordering and publish ordering.
fn member_graph<F>(members: &BTreeSet<PackageId>, outgoing_edges: &F) -> Graph<PackageId, DepKind>
where
    F: Fn(&PackageId) -> Vec<(PackageId, DepKind)>,
{
    let mut builder = GraphBuilder::<PackageId, DepKind>::with_capacity(members.len());
    for package in members {
        let node = builder.node(package.clone());
        for (dependency, kind) in outgoing_edges(package) {
            if members.contains(&dependency) {
                builder.depends_on(node, dependency, kind);
            }
        }
    }
    builder.build(UnknownTargets::Error)
}

/// One loop to show for a failed sort, each package followed by the one that depends on it and the first repeated at
/// the end. Members are distinct and every edge points at a member, so a cycle is the only thing validation reports.
fn cycle_of<K: Clone>(violations: &Violations<K>) -> Vec<K> {
    let mut cycle: Vec<K> = violations
        .iter()
        .find_map(|violation| match violation {
            Violation::Cycle(cycle) => Some(cycle.path().iter().rev().cloned().collect()),
            Violation::SelfDependency { key } => Some(vec![key.clone()]),
            _ => None,
        })
        .unwrap_or_default();
    if let Some(first) = cycle.first().cloned() {
        cycle.push(first);
    }
    cycle
}

/// Edge kinds cascade/version-bump propagation cares about: a `Dev`-only
/// dependency change correctly never forces a consumer's version to bump.
pub const CASCADE_ORDERING_KINDS: &[DepKind] = &[DepKind::Runtime, DepKind::Build, DepKind::Optional];

/// Edge kinds publish ordering cares about: cascade's kinds, plus `Dev`.
/// `cargo publish` verifies by building the packaged tarball, which needs every
/// dependency -- `[dev-dependencies]` included -- resolvable from the registry.
pub const PUBLISH_ORDERING_KINDS: &[DepKind] = &[DepKind::Runtime, DepKind::Build, DepKind::Optional, DepKind::Dev];

/// The waits between a release's distributions: each distribution's publish follows the distributions it depends on.
///
/// Built once from every dependency edge between distributions, and the cycle check, the dev edges it drops and the
/// waits it returns all come from that one graph. Mutual dev-only dependencies (cross-integration tests) are
/// legitimate, so a `Dev` edge between two distributions of the same cycle is dropped; a `Dev` edge anywhere else
/// still waits. A cycle that survives with every `Dev` edge excluded is a real `Runtime`/`Build`/`Optional` cycle and
/// fails construction with [`GraphError::Cycle`], naming the distributions' packages.
pub struct PublishWaits {
    waits: BTreeMap<ReleasePackageId, BTreeSet<ReleasePackageId>>,
}

impl PublishWaits {
    /// `distributions` pairs each distribution with its package; `edges` are `(dependent, dependency, kind)` between
    /// them. Kinds outside [`PUBLISH_ORDERING_KINDS`] do not wait.
    pub fn new(
        distributions: &BTreeMap<ReleasePackageId, PackageId>,
        edges: &[(ReleasePackageId, ReleasePackageId, DepKind)],
    ) -> Result<Self, GraphError> {
        let mut builder = GraphBuilder::<ReleasePackageId, DepKind>::with_capacity(distributions.len());
        let nodes: BTreeMap<_, _> = distributions.keys().map(|id| (id, builder.node(id.clone()))).collect();
        for (dependent, dependency, kind) in edges {
            if let Some(node) = nodes.get(dependent) {
                builder.depends_on(*node, dependency.clone(), *kind);
            }
        }
        let graph = builder.build(UnknownTargets::Error);
        graph
            .validate_filtered(|edge| CASCADE_ORDERING_KINDS.contains(edge.kind), Order::Key)
            .map_err(|violations| GraphError::Cycle {
                cycle: cycle_of(&violations)
                    .iter()
                    .filter_map(|id| distributions.get(id).cloned())
                    .collect(),
            })?;
        let dev_cycles: Vec<BTreeSet<ReleasePackageId>> = graph
            .cycles(|edge| PUBLISH_ORDERING_KINDS.contains(edge.kind), Order::Key)
            .into_iter()
            .map(|cycle| cycle.members().iter().cloned().collect())
            .collect();
        let mut waits: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        for (dependent, dependency, kind) in edges {
            let dropped = *kind == DepKind::Dev
                && dev_cycles
                    .iter()
                    .any(|cycle| cycle.contains(dependent) && cycle.contains(dependency));
            if PUBLISH_ORDERING_KINDS.contains(kind) && !dropped {
                waits.entry(dependent.clone()).or_default().insert(dependency.clone());
            }
        }
        Ok(PublishWaits { waits })
    }

    /// The distributions `distribution`'s publish waits for.
    pub fn of(&self, distribution: &ReleasePackageId) -> impl Iterator<Item = &ReleasePackageId> {
        self.waits.get(distribution).into_iter().flatten()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn test_toposort_self_loop_cycle() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let subset: HashSet<_> = vec![pkg_a.clone()].into_iter().collect();
        let all = vec![pkg_a.clone()];

        let err = toposort_impl(&subset, &all, &[DepKind::Runtime], |id| {
            if id == &pkg_a {
                vec![(pkg_a.clone(), DepKind::Runtime)]
            } else {
                vec![]
            }
        })
        .unwrap_err();

        if let GraphError::Cycle { cycle } = err {
            assert_eq!(cycle, vec![pkg_a.clone(), pkg_a]);
        } else {
            panic!("expected Cycle error");
        }
    }

    fn dist(ecosystem: &str, name: &str) -> ReleasePackageId {
        format!("{ecosystem}/{name}").parse().unwrap()
    }

    fn waits(edges: &[(ReleasePackageId, ReleasePackageId, DepKind)]) -> Result<PublishWaits, GraphError> {
        let distributions = edges
            .iter()
            .flat_map(|(from, to, _)| [from, to])
            .map(|id| (id.clone(), PackageId::parse(id.name()).unwrap()))
            .collect();
        PublishWaits::new(&distributions, edges)
    }

    #[test]
    fn a_dev_edge_waits_unless_it_closes_a_cycle() {
        let (a, b, c, d) = (
            dist("cargo", "a"),
            dist("npm", "b"),
            dist("cargo", "c"),
            dist("cargo", "d"),
        );
        let waits = waits(&[
            (a.clone(), b.clone(), DepKind::Dev),
            (b.clone(), a.clone(), DepKind::Runtime),
            (c.clone(), d.clone(), DepKind::Dev),
            (d.clone(), d.clone(), DepKind::Dev),
        ])
        .unwrap();
        assert_eq!(waits.of(&a).count(), 0);
        assert_eq!(waits.of(&b).collect::<Vec<_>>(), [&a]);
        assert_eq!(waits.of(&c).collect::<Vec<_>>(), [&d]);
        assert_eq!(waits.of(&d).count(), 0);
    }

    #[test]
    fn a_runtime_cycle_names_the_packages() {
        let (a, b) = (dist("cargo", "a"), dist("npm", "b"));
        let error = waits(&[(a.clone(), b.clone(), DepKind::Build), (b, a, DepKind::Optional)])
            .err()
            .unwrap();
        let a_b = [PackageId::parse("a").unwrap(), PackageId::parse("b").unwrap()];
        match error {
            GraphError::Cycle { cycle } => {
                assert_eq!(cycle.len(), 3, "{cycle:?}");
                assert!(cycle.iter().all(|package| a_b.contains(package)), "{cycle:?}");
            }
            other => panic!("expected a cycle, got {other:?}"),
        }
    }

    /// The Kahn pass this module used before michi-graph, kept as the oracle: `None` when it cannot order everything.
    fn reference_sort(
        members: &BTreeSet<PackageId>,
        edges: &[(PackageId, PackageId, DepKind)],
        allowed: &[DepKind],
    ) -> Option<Vec<PackageId>> {
        let mut in_degree: BTreeMap<&PackageId, usize> = members.iter().map(|m| (m, 0)).collect();
        let mut dependents: BTreeMap<&PackageId, Vec<&PackageId>> = BTreeMap::new();
        for (from, to, kind) in edges {
            if allowed.contains(kind) {
                *in_degree.get_mut(from).unwrap() += 1;
                dependents.entry(to).or_default().push(from);
            }
        }
        let mut queue: BTreeSet<&PackageId> = in_degree.iter().filter(|(_, d)| **d == 0).map(|(m, _)| *m).collect();
        let mut sorted = Vec::new();
        while let Some(next) = queue.pop_first() {
            sorted.push(next.clone());
            for dependent in dependents.get(next).into_iter().flatten() {
                let degree = in_degree.get_mut(dependent).unwrap();
                *degree -= 1;
                if *degree == 0 {
                    queue.insert(dependent);
                }
            }
        }
        (sorted.len() == members.len()).then_some(sorted)
    }

    proptest::proptest! {
        /// Random graphs, cyclic or not, with random edge kinds: the sort agrees with the previous implementation on
        /// whether it succeeds and on the exact order, and a failure names a loop whose members all take part in it.
        #[test]
        fn the_sort_matches_the_previous_implementation(
            size in 1usize..10,
            raw in proptest::collection::vec((0usize..10, 0usize..10, 0u8..4), 0..30),
        ) {
            let ids: Vec<PackageId> = (0..size).map(|i| PackageId::parse(&format!("pkg-{i}")).unwrap()).collect();
            let kinds = [DepKind::Runtime, DepKind::Build, DepKind::Optional, DepKind::Dev];
            let members: BTreeSet<PackageId> = ids.iter().cloned().collect();
            let edges: Vec<(PackageId, PackageId, DepKind)> = raw
                .into_iter()
                .map(|(a, b, k)| (ids[a % size].clone(), ids[b % size].clone(), kinds[usize::from(k)]))
                .collect();
            let subset: HashSet<PackageId> = members.iter().cloned().collect();
            let result = toposort_impl(&subset, &ids, CASCADE_ORDERING_KINDS, |id| {
                edges.iter().filter(|(from, _, _)| from == id).map(|(_, to, kind)| (to.clone(), *kind)).collect()
            });
            match reference_sort(&members, &edges, CASCADE_ORDERING_KINDS) {
                Some(expected) => proptest::prop_assert_eq!(result.unwrap(), expected),
                None => match result {
                    Err(GraphError::Cycle { cycle }) => {
                        proptest::prop_assert!(cycle.len() >= 2);
                        proptest::prop_assert_eq!(cycle.first(), cycle.last());
                        proptest::prop_assert!(cycle.iter().all(|member| members.contains(member)));
                    }
                    other => proptest::prop_assert!(false, "expected a cycle, got {:?}", other),
                },
            }
        }
    }

    #[test]
    fn a_failed_sort_names_the_loop_in_dependency_to_dependent_order() {
        let (a, b) = (PackageId::parse("pkg-a").unwrap(), PackageId::parse("pkg-b").unwrap());
        let subset: HashSet<_> = [a.clone(), b.clone()].into_iter().collect();
        let all = vec![a.clone(), b.clone()];
        let error = toposort_impl(&subset, &all, &[DepKind::Runtime], |id| {
            vec![(if id == &a { b.clone() } else { a.clone() }, DepKind::Runtime)]
        })
        .unwrap_err();
        match error {
            GraphError::Cycle { cycle } => {
                assert_eq!(cycle.len(), 3, "{cycle:?}");
                assert_eq!(cycle.first(), cycle.last());
            }
            other => panic!("expected a cycle, got {other:?}"),
        }
    }
}
