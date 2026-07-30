use std::collections::{BTreeMap, BTreeSet, HashSet};

use callisto_model::{DepKind, PackageId};

use crate::error::GraphError;

pub fn toposort_impl<F>(
    subset: &HashSet<PackageId>,
    all_packages: &[PackageId],
    outgoing_edges: F,
) -> Result<Vec<PackageId>, GraphError>
where
    F: Fn(&PackageId) -> Vec<(PackageId, DepKind)>,
{
    let members: BTreeSet<PackageId> = subset.iter().cloned().collect();
    for id in &members {
        if !all_packages.contains(id) {
            return Err(GraphError::UnknownPackage { id: id.clone() });
        }
    }

    let mut in_degree: BTreeMap<PackageId, usize> = BTreeMap::new();
    let mut adj: BTreeMap<PackageId, Vec<PackageId>> = BTreeMap::new();

    for u in &members {
        in_degree.insert(u.clone(), 0);
        adj.insert(u.clone(), Vec::new());
    }

    for u in &members {
        for (v, kind) in outgoing_edges(u) {
            if members.contains(&v)
                && matches!(kind, DepKind::Runtime | DepKind::Build | DepKind::Optional)
            {
                adj.get_mut(&v).unwrap().push(u.clone());
                *in_degree.get_mut(u).unwrap() += 1;
            }
        }
    }

    let mut queue: BTreeSet<PackageId> = BTreeSet::new();
    for (u, &deg) in &in_degree {
        if deg == 0 {
            queue.insert(u.clone());
        }
    }

    let mut sorted = Vec::new();
    while let Some(u) = queue.pop_first() {
        sorted.push(u.clone());
        if let Some(neighbors) = adj.get(&u) {
            for v in neighbors {
                let deg = in_degree.get_mut(v).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    queue.insert(v.clone());
                }
            }
        }
    }

    if sorted.len() < members.len() {
        let set: HashSet<PackageId> = sorted.iter().cloned().collect();
        let remaining: Vec<PackageId> = members.into_iter().filter(|m| !set.contains(m)).collect();
        let cycle = extract_cycle(&remaining, &adj);
        return Err(GraphError::Cycle { cycle });
    }

    Ok(sorted)
}

fn extract_cycle(
    remaining: &[PackageId],
    adj: &BTreeMap<PackageId, Vec<PackageId>>,
) -> Vec<PackageId> {
    use petgraph::algo::tarjan_scc;
    use petgraph::graph::DiGraph;

    let mut graph = DiGraph::<PackageId, ()>::new();
    let mut node_map = BTreeMap::new();
    let mut rev_map = BTreeMap::new();

    for pkg in remaining {
        let idx = graph.add_node(pkg.clone());
        node_map.insert(pkg.clone(), idx);
        rev_map.insert(idx, pkg.clone());
    }

    for u in remaining {
        if let Some(neighbors) = adj.get(u) {
            for v in neighbors {
                if let (Some(&u_idx), Some(&v_idx)) = (node_map.get(u), node_map.get(v)) {
                    graph.add_edge(u_idx, v_idx, ());
                }
            }
        }
    }

    let sccs = tarjan_scc(&graph);
    for scc in sccs {
        let is_self_loop = scc.len() == 1 && graph.contains_edge(scc[0], scc[0]);
        if scc.len() > 1 || is_self_loop {
            let mut cycle = Vec::new();
            let start_idx = scc[0];
            let mut curr = start_idx;
            let scc_set: std::collections::HashSet<_> = scc.iter().copied().collect();

            cycle.push(rev_map[&curr].clone());
            loop {
                let mut next_found = None;
                for neighbor in graph.neighbors(curr) {
                    if scc_set.contains(&neighbor) {
                        next_found = Some(neighbor);
                        break;
                    }
                }

                if let Some(next) = next_found {
                    curr = next;
                    cycle.push(rev_map[&curr].clone());
                    if curr == start_idx {
                        break;
                    }
                } else {
                    break;
                }
            }

            if cycle.len() > 1 {
                return cycle;
            }
        }
    }

    remaining.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toposort_linear() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let pkg_b = PackageId::parse("pkg-b").unwrap();

        let subset: HashSet<_> = vec![pkg_a.clone(), pkg_b.clone()].into_iter().collect();
        let all = vec![pkg_a.clone(), pkg_b.clone()];

        let res = toposort_impl(&subset, &all, |id| {
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
    fn test_toposort_self_loop_cycle() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let subset: HashSet<_> = vec![pkg_a.clone()].into_iter().collect();
        let all = vec![pkg_a.clone()];

        let err = toposort_impl(&subset, &all, |id| {
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
}
