mod fixtures;
use callisto_graph::cascade::{run_cascade, CascadeInput};
use callisto_graph::config::groups::GroupTable;
use callisto_graph::config::{CascadeBumpSeverity, CascadeConfig, CascadeMode};
use callisto_model::{DepKind, DepSpec, PackageId, Severity, Version, VersionReq};
use fixtures::GraphBuilder;
use std::collections::BTreeMap;

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
fn test_absolute_path_workspace_cargo_resolver() {
    let temp_dir = tempfile::tempdir().unwrap();
    let root_cargo = temp_dir.path().join("Cargo.toml");
    let content = r#"[workspace]
members = ["crates/sub"]
resolver = "2"

[workspace.package]
version = "0.2.0"
"#;
    std::fs::write(&root_cargo, content).unwrap();

    // Must load without error when passed an absolute path
    let resolver = callisto_manifests::WorkspaceCargoResolver::load(&root_cargo);
    assert!(resolver.is_ok());

    let inh = resolver.unwrap().inheritance().unwrap();
    assert_eq!(inh.version.unwrap().render(), "0.2.0");
}

#[test]
fn test_leading_dash_package_id_rejection() {
    let result = PackageId::parse("-x");
    assert!(result.is_err());
}

#[test]
fn test_atomic_write_utility() {
    let temp_dir = tempfile::tempdir().unwrap();
    let target_file = temp_dir.path().join("atomic_test.txt");
    let content = "callisto atomic write test payload\n";

    callisto_manifests::atomic::atomic_write(&target_file, content).unwrap();

    assert!(target_file.exists());
    let read_back = std::fs::read_to_string(&target_file).unwrap();
    assert_eq!(read_back, content);
}

#[test]
fn test_validate_detects_empty_changesets() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cs_dir = temp_dir.path().join(".changeset");
    std::fs::create_dir_all(&cs_dir).unwrap();
    std::fs::write(cs_dir.join("empty.md"), "---\n---\n").unwrap();

    let cfg = callisto_graph::config::load(&temp_dir.path().join("callisto.toml")).unwrap();
    let loaded = callisto_graph::load_changesets(temp_dir.path(), &cfg);
    assert!(loaded.is_err());
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

    // Spec §G.6.7: both linked packages marked minor must converge to 2.8.0
    assert_eq!(target_a, "2.8.0");
    assert_eq!(target_b, "2.8.0");
}
