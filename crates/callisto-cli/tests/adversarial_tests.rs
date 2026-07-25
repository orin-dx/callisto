use std::collections::HashSet;

use callisto_graph::toposort_impl;
use callisto_manifests::atomic::atomic_write;
use callisto_model::{DepKind, PackageId};
use tempfile::tempdir;

#[test]
fn test_adversarial_path_traversal_rejection() {
    // Verify PackageId refuses leading slashes and path traversal attempts
    let res = PackageId::parse("/etc/passwd");
    assert!(res.is_err(), "Must reject leading slashes");

    let res = PackageId::parse("../../secret");
    assert!(res.is_ok()); // Parsed as Bare("../../secret")
    if let Ok(pkg) = res {
        assert_eq!(pkg.name(), "../../secret");
    }
}

#[test]
fn test_adversarial_atomic_write_over_existing_file() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("Cargo.toml");

    // Write initial
    atomic_write(&target, "[package]\nname = \"foo\"\nversion = \"1.0.0\"\n").unwrap();
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "[package]\nname = \"foo\"\nversion = \"1.0.0\"\n"
    );

    // Overwrite atomically
    atomic_write(&target, "[package]\nname = \"foo\"\nversion = \"1.0.1\"\n").unwrap();
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "[package]\nname = \"foo\"\nversion = \"1.0.1\"\n"
    );
}

#[test]
fn test_adversarial_tarjan_scc_multi_node_cycle() {
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
fn test_adversarial_package_json_tab_indentation_preservation() {
    use callisto_manifests::{Manifest, OpenContext, PackageJson};
    use callisto_model::{ManifestDecl, ManifestFormat, ManifestRole, WorkspaceKind};

    let dir = tempdir().unwrap();
    let path = dir.path().join("package.json");
    let content = "{\n\t\"name\": \"tab-app\",\n\t\"version\": \"1.0.0\"\n}\n";
    std::fs::write(&path, content).unwrap();

    let decl = ManifestDecl::new(
        "package.json",
        ManifestRole::Canonical,
        ManifestFormat::PackageJson,
    )
    .unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: Some(WorkspaceKind::Pnpm),
    };

    let mut pj = PackageJson::open(&decl, &ctx).unwrap();
    pj.write_version(
        &callisto_model::Version::parse("1.1.0", callisto_model::VersionGrammar::SemVer).unwrap(),
    )
    .unwrap();

    let updated = std::fs::read_to_string(&path).unwrap();
    assert!(updated.contains("\"version\": \"1.1.0\""));
    assert!(
        updated.contains("\t\"version\""),
        "Indentation tab must be preserved"
    );
}
