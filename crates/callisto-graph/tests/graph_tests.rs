use callisto_fixtures::GraphBuilder;
use callisto_graph::cascade::{run_cascade, CascadeInput};
use callisto_graph::config::groups::GroupTable;
use callisto_graph::config::{CascadeBumpSeverity, CascadeConfig, CascadeMode};
use callisto_model::{DepKind, DepSpec, PackageId, Severity, Version, VersionReq};
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
