use std::collections::{BTreeSet, HashSet};

use callisto_model::{DepKind, PackageId};
use michi_graph::{Graph, GraphBuilder, Order, UnknownTargets, Violation, Violations};

use crate::error::GraphError;

/// Topologically sorts `subset` by the edges `outgoing_edges` reports, counting only edges
/// whose [`DepKind`] appears in `ordering_kinds` as ordering constraints. Different callers
/// need different kinds to count: cascade/version-bump propagation only cares about
/// `Runtime`/`Build`/`Optional` (a `Dev`-only change correctly shouldn't force a consumer's
/// version to bump), while publish ordering ([`PublishEdgeFilter`]) also needs
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
    toposort_with_edge_filter(subset, all_packages, outgoing_edges, |_from, _to, kind| {
        ordering_kinds.contains(&kind)
    })
}

/// Same algorithm as [`toposort_impl`], but the caller decides per-edge
/// whether it counts as an ordering constraint, rather than a single
/// `DepKind` list applied uniformly to every pair. This is what lets
/// a caller exclude a `Dev` edge only between the
/// specific packages that form a cycle, instead of dropping `Dev` ordering
/// for every package in `subset` the moment any one Dev cycle exists.
pub fn toposort_with_edge_filter<F, P>(
    subset: &HashSet<PackageId>,
    all_packages: &[PackageId],
    outgoing_edges: F,
    edge_allowed: P,
) -> Result<Vec<PackageId>, GraphError>
where
    F: Fn(&PackageId) -> Vec<(PackageId, DepKind)>,
    P: Fn(&PackageId, &PackageId, DepKind) -> bool,
{
    let members: BTreeSet<PackageId> = subset.iter().cloned().collect();
    // Build a HashSet once so validation is O(N) rather than O(N²) slice scans.
    let all_set: std::collections::HashSet<&PackageId> = all_packages.iter().collect();
    for id in &members {
        if !all_set.contains(id) {
            return Err(GraphError::UnknownPackage { id: id.clone() });
        }
    }

    let dag = member_graph(&members, &outgoing_edges)
        .validate_filtered(
            |edge| edge_allowed(edge.dependent, edge.prerequisite, *edge.kind),
            Order::Key,
        )
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
fn cycle_of(violations: &Violations<PackageId>) -> Vec<PackageId> {
    let mut cycle: Vec<PackageId> = violations
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

/// Which dependency edges order a publish: every [`PUBLISH_ORDERING_KINDS`] edge,
/// except a `Dev` edge between two packages of the same cycle.
///
/// Mutual dev-only dependencies (cross-integration tests) are legitimate, so a
/// `Dev` edge inside a cyclic component is dropped; a `Dev` edge anywhere else
/// still orders. A cycle that survives with every `Dev` edge excluded is a real
/// `Runtime`/`Build`/`Optional` cycle and fails construction with
/// [`GraphError::Cycle`]. Used by release derivation.
pub struct PublishEdgeFilter {
    dev_cycles: Vec<HashSet<PackageId>>,
}

impl PublishEdgeFilter {
    pub fn new<F>(
        subset: &HashSet<PackageId>,
        all_packages: &[PackageId],
        outgoing_edges: F,
    ) -> Result<Self, GraphError>
    where
        F: Fn(&PackageId) -> Vec<(PackageId, DepKind)>,
    {
        toposort_impl(subset, all_packages, CASCADE_ORDERING_KINDS, &outgoing_edges)?;
        Ok(PublishEdgeFilter {
            dev_cycles: cyclic_sccs(subset, &outgoing_edges, PUBLISH_ORDERING_KINDS),
        })
    }

    /// Whether the `from -> to` edge of `kind` is a publish-ordering constraint.
    pub fn allows(&self, from: &PackageId, to: &PackageId, kind: DepKind) -> bool {
        if !PUBLISH_ORDERING_KINDS.contains(&kind) {
            return false;
        }
        kind != DepKind::Dev
            || !self
                .dev_cycles
                .iter()
                .any(|component| component.contains(from) && component.contains(to))
    }
}

/// Returns every non-trivial strongly-connected component (size > 1, or a
/// single self-looping node) of `subset` under the edges `ordering_kinds`
/// selects — the full member set, not just what's left after a failed
/// toposort. Used by [`PublishEdgeFilter`] to scope a Dev-cycle
/// exclusion to exactly the packages that participate in a cycle, rather
/// than excluding Dev edges across the whole subset.
pub fn cyclic_sccs<F>(
    subset: &HashSet<PackageId>,
    outgoing_edges: F,
    ordering_kinds: &[DepKind],
) -> Vec<HashSet<PackageId>>
where
    F: Fn(&PackageId) -> Vec<(PackageId, DepKind)>,
{
    let members: BTreeSet<PackageId> = subset.iter().cloned().collect();
    member_graph(&members, &outgoing_edges)
        .cycles(|edge| ordering_kinds.contains(edge.kind), Order::Key)
        .into_iter()
        .map(|cycle| cycle.members().iter().cloned().collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn test_toposort_linear() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let pkg_b = PackageId::parse("pkg-b").unwrap();

        let subset: HashSet<_> = vec![pkg_a.clone(), pkg_b.clone()].into_iter().collect();
        let all = vec![pkg_a.clone(), pkg_b.clone()];

        let res = toposort_impl(&subset, &all, &[DepKind::Runtime], |id| {
            if id == &pkg_a {
                vec![(pkg_b.clone(), DepKind::Runtime)]
            } else {
                vec![]
            }
        })
        .unwrap();

        assert_eq!(res, vec![pkg_b, pkg_a]);
    }

    #[test]
    fn test_toposort_produces_correct_order_large_graph() {
        // Build a 10-package chain: pkg-a -> pkg-b -> ... -> pkg-i -> pkg-j
        // Each package depends on the next; pkg-j is the leaf with no dependencies.
        // Expected toposort order (dependencies first): [pkg-j, pkg-i, ..., pkg-a]
        let ids: Vec<PackageId> = (b'a'..=b'j')
            .map(|c| PackageId::parse(&format!("pkg-{}", c as char)).unwrap())
            .collect();

        let subset: HashSet<PackageId> = ids.iter().cloned().collect();
        let all: Vec<PackageId> = ids.clone();

        let result = toposort_impl(&subset, &all, &[DepKind::Runtime], |id| {
            // Find the index of this package; if it's not the last, it depends on the next.
            if let Some(pos) = ids.iter().position(|x| x == id)
                && pos + 1 < ids.len()
            {
                return vec![(ids[pos + 1].clone(), DepKind::Runtime)];
            }
            vec![]
        })
        .unwrap();

        // pkg-j has no dependents so it sorts first; pkg-a sorts last.
        let expected: Vec<PackageId> = ids.iter().cloned().rev().collect();
        assert_eq!(result, expected);
    }

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

    #[test]
    fn cyclic_sccs_finds_only_nontrivial_components() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let pkg_b = PackageId::parse("pkg-b").unwrap();
        let pkg_c = PackageId::parse("pkg-c").unwrap();
        let pkg_d = PackageId::parse("pkg-d").unwrap();

        // pkg-a <-> pkg-b: a real 2-node cycle. pkg-c -> pkg-d: acyclic, no
        // cycle at all.
        let subset: HashSet<_> = vec![pkg_a.clone(), pkg_b.clone(), pkg_c.clone(), pkg_d.clone()]
            .into_iter()
            .collect();

        let sccs = cyclic_sccs(
            &subset,
            |id| {
                if id == &pkg_a {
                    vec![(pkg_b.clone(), DepKind::Dev)]
                } else if id == &pkg_b {
                    vec![(pkg_a.clone(), DepKind::Dev)]
                } else if id == &pkg_c {
                    vec![(pkg_d.clone(), DepKind::Dev)]
                } else {
                    vec![]
                }
            },
            &[DepKind::Dev],
        );

        assert_eq!(sccs.len(), 1, "expected exactly one cyclic component; got {sccs:?}");
        assert_eq!(sccs[0], vec![pkg_a, pkg_b].into_iter().collect::<HashSet<_>>());
    }

    #[test]
    fn cyclic_sccs_includes_self_loop() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let subset: HashSet<_> = vec![pkg_a.clone()].into_iter().collect();

        let sccs = cyclic_sccs(&subset, |id| vec![(id.clone(), DepKind::Dev)], &[DepKind::Dev]);

        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0], vec![pkg_a].into_iter().collect::<HashSet<_>>());
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
