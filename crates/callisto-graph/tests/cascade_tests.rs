mod fixtures;
use callisto_graph::cascade::{run_cascade, CascadeInput};
use callisto_graph::config::groups::GroupTable;
use callisto_graph::config::{CascadeBumpSeverity, CascadeConfig, CascadeMode};
use callisto_graph::toposort_impl;
use callisto_model::{DepKind, DepSpec, PackageId, Severity, Version, VersionGrammar, VersionReq};
use fixtures::GraphBuilder;
use std::collections::{BTreeMap, HashSet};

#[test]
fn test_blackbox_cascade_propagation() {
    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();

    let graph = GraphBuilder::new()
        .package(pkg_a.clone(), |p| p)
        .package(pkg_b.clone(), |p| p)
        .edge(
            pkg_a.clone(),
            pkg_b.clone(),
            DepKind::Runtime,
            DepSpec::Range(
                VersionReq::parse("^1.0.0", callisto_model::Ecosystem::Cargo).unwrap(),
                "^1.0.0".to_string(),
            ),
        )
        .build()
        .unwrap();

    let groups = GroupTable::default();
    let cfg = CascadeConfig {
        mode: CascadeMode::OutOfRange,
        bump_severity: CascadeBumpSeverity::Patch,
        peer_escalation: true,
        preserve_npm_ranges: false,
    };

    let mut seed = BTreeMap::new();
    seed.insert(pkg_b.clone(), Severity::Major);

    let mut base = BTreeMap::new();
    base.insert(pkg_a.clone(), Version::semver(1, 0, 0));
    base.insert(pkg_b.clone(), Version::semver(1, 0, 0));

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
    assert_eq!(outcome.severities.get(&pkg_b), Some(&Severity::Major));
    // pkg_a depends on pkg_b via ^1.0.0; after pkg_b bumps to 2.x the
    // constraint goes out of range, so cascade must propagate a bump to pkg_a.
    assert_eq!(
        outcome.severities.get(&pkg_a),
        Some(&Severity::Patch),
        "cascade must propagate a Patch bump to pkg_a which depends on pkg_b"
    );
}

#[test]
fn test_diamond_cascade_convergence() {
    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();
    let pkg_c = PackageId::parse("pkg-c").unwrap();
    let pkg_d = PackageId::parse("pkg-d").unwrap();

    let graph = GraphBuilder::new()
        .package(pkg_a.clone(), |p| p)
        .package(pkg_b.clone(), |p| p)
        .package(pkg_c.clone(), |p| p)
        .package(pkg_d.clone(), |p| p)
        .edge(
            pkg_a.clone(),
            pkg_b.clone(),
            DepKind::Runtime,
            DepSpec::Range(
                VersionReq::parse("^1.0.0", callisto_model::Ecosystem::Cargo).unwrap(),
                "^1.0.0".to_string(),
            ),
        )
        .edge(
            pkg_a.clone(),
            pkg_c.clone(),
            DepKind::Runtime,
            DepSpec::Range(
                VersionReq::parse("^1.0.0", callisto_model::Ecosystem::Cargo).unwrap(),
                "^1.0.0".to_string(),
            ),
        )
        .edge(
            pkg_b.clone(),
            pkg_d.clone(),
            DepKind::Runtime,
            DepSpec::Range(
                VersionReq::parse("^1.0.0", callisto_model::Ecosystem::Cargo).unwrap(),
                "^1.0.0".to_string(),
            ),
        )
        .edge(
            pkg_c.clone(),
            pkg_d.clone(),
            DepKind::Runtime,
            DepSpec::Range(
                VersionReq::parse("^1.0.0", callisto_model::Ecosystem::Cargo).unwrap(),
                "^1.0.0".to_string(),
            ),
        )
        .build()
        .unwrap();

    let groups = GroupTable::default();
    let cfg = CascadeConfig {
        mode: CascadeMode::OutOfRange,
        bump_severity: CascadeBumpSeverity::Patch,
        peer_escalation: true,
        preserve_npm_ranges: false,
    };

    let mut seed = BTreeMap::new();
    seed.insert(pkg_d.clone(), Severity::Major);

    let mut base = BTreeMap::new();
    base.insert(pkg_a.clone(), Version::semver(1, 0, 0));
    base.insert(pkg_b.clone(), Version::semver(1, 0, 0));
    base.insert(pkg_c.clone(), Version::semver(1, 0, 0));
    base.insert(pkg_d.clone(), Version::semver(1, 0, 0));

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
    assert_eq!(outcome.severities.get(&pkg_d), Some(&Severity::Major));
    assert!(outcome.severities.get(&pkg_b).unwrap() >= &Severity::Patch);
    assert!(outcome.severities.get(&pkg_c).unwrap() >= &Severity::Patch);
}

#[test]
fn test_peer_dependency_escalation() {
    let pkg_app = PackageId::parse("pkg-app").unwrap();
    let pkg_plugin = PackageId::parse("pkg-plugin").unwrap();

    let graph = GraphBuilder::new()
        .package(pkg_app.clone(), |p| p)
        .package(pkg_plugin.clone(), |p| p)
        .edge(
            pkg_plugin.clone(),
            pkg_app.clone(),
            DepKind::Peer,
            DepSpec::Range(
                VersionReq::parse("^1.0.0", callisto_model::Ecosystem::Npm).unwrap(),
                "^1.0.0".to_string(),
            ),
        )
        .build()
        .unwrap();

    let groups = GroupTable::default();
    let cfg = CascadeConfig {
        mode: CascadeMode::OutOfRange,
        bump_severity: CascadeBumpSeverity::Patch,
        peer_escalation: true,
        preserve_npm_ranges: false,
    };

    let mut seed = BTreeMap::new();
    seed.insert(pkg_app.clone(), Severity::Major);

    let mut base = BTreeMap::new();
    base.insert(pkg_app.clone(), Version::semver(1, 0, 0));
    base.insert(pkg_plugin.clone(), Version::semver(1, 0, 0));

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
    assert_eq!(outcome.severities.get(&pkg_plugin), Some(&Severity::Major));
}

#[test]
fn test_linked_group_converges_shared_version() {
    use callisto_graph::config::{GroupDef, GroupMember};
    use callisto_model::{GroupKind, GroupName};

    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();

    let graph = GraphBuilder::new()
        .package(pkg_a.clone(), |p| p)
        .package(pkg_b.clone(), |p| p)
        .build()
        .unwrap();

    let mut groups = GroupTable::default();
    let group_name = GroupName("group-ab".to_string());
    groups.linked.insert(
        group_name.clone(),
        GroupDef {
            name: group_name.clone(),
            kind: GroupKind::Linked,
            members: vec![
                GroupMember::Package(pkg_a.clone()),
                GroupMember::Package(pkg_b.clone()),
            ],
        },
    );
    groups.linked_of.insert(pkg_a.clone(), group_name.clone());
    groups.linked_of.insert(pkg_b.clone(), group_name);

    let cfg = CascadeConfig {
        mode: CascadeMode::OutOfRange,
        bump_severity: CascadeBumpSeverity::Patch,
        peer_escalation: true,
        preserve_npm_ranges: false,
    };

    let mut seed = BTreeMap::new();
    seed.insert(pkg_a.clone(), Severity::Patch);

    let mut base = BTreeMap::new();
    base.insert(pkg_a.clone(), Version::semver(1, 0, 0));
    base.insert(pkg_b.clone(), Version::semver(2, 0, 0));

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

    // Spec §G.6.7: Linked group syncs release severities across members, and
    // members converge on a single winning target version (the max of each
    // member's individually-computed candidate) rather than diverging by
    // their own base version: pkg-a's candidate is 1.0.1, pkg-b's is 2.0.1,
    // so both converge on the winner, 2.0.1.
    assert_eq!(outcome.severities.get(&pkg_a), Some(&Severity::Patch));
    assert_eq!(outcome.severities.get(&pkg_b), Some(&Severity::Patch));
    assert_eq!(outcome.targets.get(&pkg_a), Some(&Version::semver(2, 0, 1)));
    assert_eq!(outcome.targets.get(&pkg_b), Some(&Version::semver(2, 0, 1)));
}

#[test]
fn test_linked_group_version_convergence() {
    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();

    let graph = GraphBuilder::new()
        .package(pkg_a.clone(), |p| p)
        .package(pkg_b.clone(), |p| p)
        .build()
        .unwrap();

    let mut base = BTreeMap::new();
    base.insert(pkg_a.clone(), Version::semver(1, 4, 0));
    base.insert(pkg_b.clone(), Version::semver(2, 7, 3));

    let mut initial_severities = BTreeMap::new();
    initial_severities.insert(pkg_a.clone(), Severity::Minor);
    initial_severities.insert(pkg_b.clone(), Severity::Minor);

    let mut groups = GroupTable::default();
    let mut group_def = callisto_graph::config::GroupDef {
        name: callisto_model::GroupName("core_linked".to_string()),
        kind: callisto_model::GroupKind::Linked,
        members: Vec::new(),
    };
    group_def
        .members
        .push(callisto_graph::config::GroupMember::Package(pkg_a.clone()));
    group_def
        .members
        .push(callisto_graph::config::GroupMember::Package(pkg_b.clone()));
    groups.linked.insert(group_def.name.clone(), group_def);

    let cfg = CascadeConfig {
        mode: CascadeMode::OutOfRange,
        bump_severity: CascadeBumpSeverity::Patch,
        peer_escalation: true,
        preserve_npm_ranges: false,
    };
    let named_by = BTreeMap::new();
    let reasons = BTreeMap::new();

    let input = CascadeInput {
        graph: &graph,
        groups: &groups,
        cfg: &cfg,
        seed: &initial_severities,
        reasons: &reasons,
        named_by: &named_by,
        base: &base,
        pre: None,
    };

    let outcome = run_cascade(input).unwrap();
    let target_a = outcome.targets.get(&pkg_a).unwrap().render();
    let target_b = outcome.targets.get(&pkg_b).unwrap().render();

    // Spec §G.6.7: Linked group members sync release severity (Minor) AND
    // converge on the single winning target version: pkg_a's candidate is
    // 1.5.0, pkg_b's is 2.8.0, so both converge on the winner, 2.8.0.
    assert_eq!(target_a, "2.8.0");
    assert_eq!(target_b, "2.8.0");
    assert_eq!(outcome.severities.get(&pkg_a), Some(&Severity::Minor));
    assert_eq!(outcome.severities.get(&pkg_b), Some(&Severity::Minor));
}

#[test]
fn test_tarjan_scc_detects_cycles() {
    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();
    let pkg_c = PackageId::parse("pkg-c").unwrap();

    let subset: HashSet<_> = vec![pkg_a.clone(), pkg_b.clone(), pkg_c.clone()]
        .into_iter()
        .collect();
    let all = vec![pkg_a.clone(), pkg_b.clone(), pkg_c.clone()];

    // A -> B -> C -> A
    let res = toposort_impl(&subset, &all, |id| {
        if id == &pkg_a {
            vec![(pkg_b.clone(), DepKind::Runtime)]
        } else if id == &pkg_b {
            vec![(pkg_c.clone(), DepKind::Runtime)]
        } else {
            vec![(pkg_a.clone(), DepKind::Runtime)]
        }
    });

    assert!(res.is_err());
    let err = res.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("cycle") || err_str.contains("circular"),
        "Error must diagnose cycle: {err_str}"
    );
}

#[test]
fn test_pep440_package_bump_succeeds() {
    // Regression: bump_target previously always used SemVerVersioning, which
    // returns Err(BumpError::NotSemVer) for PEP 440 versions. After the fix it
    // dispatches to Pep440Versioning and produces Ok.
    let pkg_py = PackageId::parse("my-python-lib").unwrap();

    let graph = GraphBuilder::new()
        .package(pkg_py.clone(), |p| p)
        .build()
        .unwrap();

    let groups = GroupTable::default();
    let cfg = CascadeConfig {
        mode: CascadeMode::OutOfRange,
        bump_severity: CascadeBumpSeverity::Patch,
        peer_escalation: false,
        preserve_npm_ranges: false,
    };

    let mut seed = BTreeMap::new();
    seed.insert(pkg_py.clone(), Severity::Patch);

    let mut base = BTreeMap::new();
    base.insert(
        pkg_py.clone(),
        Version::parse("1.0.0", VersionGrammar::Pep440).unwrap(),
    );

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

    // Before the fix this returns Err because SemVerVersioning rejects PEP 440 versions.
    let outcome = run_cascade(input).expect("cascade must succeed for a PEP 440 package");
    let target = outcome
        .targets
        .get(&pkg_py)
        .expect("target version must be present");
    assert_eq!(
        target.render(),
        "1.0.1",
        "PEP 440 patch bump of 1.0.0 must produce 1.0.1"
    );
}
