mod fixtures;
use callisto_graph::commands::{matrix, MatrixOptions};
use callisto_model::PackageId;
use fixtures::{GraphBuilder, PackageBuilder};
use std::cell::OnceCell;

struct DummyRunner;
impl callisto_model::CommandRunner for DummyRunner {
    fn run(
        &self,
        _program: &str,
        _args: &[&str],
        _cwd: &std::path::Path,
    ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
        Ok(callisto_model::CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

fn napi_manifest_decl(name: &str) -> callisto_model::ManifestDecl {
    callisto_model::ManifestDecl::new(
        std::path::PathBuf::from(format!("{name}/package.json")),
        callisto_model::ManifestRole::Canonical,
        callisto_model::ManifestFormat::PackageJson,
    )
    .unwrap()
}

/// --package restricts BOTH platformTargets AND runtimeVersions to
/// exactly that one package's entries.
#[test]
fn matrix_package_filter_restricts_to_one_package() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for name in ["pkg-a", "pkg-b"] {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"napi":{"targets":["aarch64-apple-darwin"]},"engines":{"node":">=20.0.0"}}"#,
        )
        .unwrap();
    }
    std::fs::write(root.join("callisto.toml"), "").unwrap();

    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();
    let graph = GraphBuilder::new()
        .package(pkg_a.clone(), |p: PackageBuilder| {
            p.manifests(vec![napi_manifest_decl("pkg-a")])
        })
        .package(pkg_b.clone(), |p: PackageBuilder| {
            p.manifests(vec![napi_manifest_decl("pkg-b")])
        })
        .build()
        .unwrap();

    let cfg = callisto_graph::config::load(root).unwrap();
    let runner = DummyRunner;
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::new(),
        git: OnceCell::new(),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let opts = MatrixOptions {
        package: Some("pkg-a".to_string()),
        ..Default::default()
    };
    let report = matrix(&ws, &opts).expect("matrix should succeed");

    assert_eq!(report.platform_targets.len(), 1);
    assert!(report.platform_targets.contains_key("pkg-a"));
    assert!(!report.platform_targets.contains_key("pkg-b"));

    assert_eq!(report.runtime_versions.len(), 1);
    assert!(
        report.runtime_versions.contains_key("pkg-a"),
        "runtimeVersions must also be restricted to pkg-a"
    );
    assert!(!report.runtime_versions.contains_key("pkg-b"));
}

/// An unknown --package name is a hard error naming the package,
/// never a structurally valid empty report.
#[test]
fn matrix_unknown_package_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("callisto.toml"), "").unwrap();

    let graph = GraphBuilder::new().build().unwrap();
    let cfg = callisto_graph::config::load(root).unwrap();
    let runner = DummyRunner;
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::new(),
        git: OnceCell::new(),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let opts = MatrixOptions {
        package: Some("does-not-exist".to_string()),
        ..Default::default()
    };
    let err = matrix(&ws, &opts).unwrap_err();
    match err {
        callisto_graph::GraphError::UnknownPackage { id } => {
            assert_eq!(id.name(), "does-not-exist");
        }
        other => panic!("expected UnknownPackage, got {other:?}"),
    }
}

/// Writes the npm package's `package.json` (napi targets) and a
/// `callisto.toml` `[[release.artifact]]` binding the cargo package's
/// binary -- the common napi split layout: a Cargo crate and an npm package
/// sharing the bare name `"shared"`, each its own registered `PackageId`.
fn write_same_name_cross_ecosystem_fixture(root: &std::path::Path) {
    let npm_dir = root.join("packages/shared");
    std::fs::create_dir_all(&npm_dir).unwrap();
    std::fs::write(
        npm_dir.join("package.json"),
        r#"{"name":"shared","napi":{"targets":["aarch64-apple-darwin"]}}"#,
    )
    .unwrap();

    std::fs::write(
        root.join("callisto.toml"),
        "[release]\nproduct-package = \"cargo/shared\"\nforge-repository = \"example/shared\"\n\n\
         [[release.artifact]]\npackage = \"cargo/shared\"\ntarget = \"x86_64-unknown-linux-gnu\"\nasset-name = \"shared.tar.gz\"\n",
    )
    .unwrap();
}

fn same_name_cross_ecosystem_ids() -> (PackageId, PackageId) {
    (
        PackageId::Prefixed {
            ecosystem: callisto_model::Ecosystem::Cargo,
            name: "shared".to_string(),
        },
        PackageId::Prefixed {
            ecosystem: callisto_model::Ecosystem::Npm,
            name: "shared".to_string(),
        },
    )
}

fn same_name_cross_ecosystem_graph(cargo_id: PackageId, npm_id: PackageId) -> fixtures::InMemoryGraph {
    let cargo_manifest = callisto_model::ManifestDecl::new(
        std::path::PathBuf::from("crates/shared/Cargo.toml"),
        callisto_model::ManifestRole::Canonical,
        callisto_model::ManifestFormat::CargoToml,
    )
    .unwrap();
    let npm_manifest = callisto_model::ManifestDecl::new(
        std::path::PathBuf::from("packages/shared/package.json"),
        callisto_model::ManifestRole::Canonical,
        callisto_model::ManifestFormat::PackageJson,
    )
    .unwrap();

    GraphBuilder::new()
        .package(cargo_id, |p: PackageBuilder| p.manifests(vec![cargo_manifest]))
        .package(npm_id, |p: PackageBuilder| p.manifests(vec![npm_manifest]))
        .build()
        .unwrap()
}

/// Both same-named packages appear, each under its own ecosystem-qualified
/// key, and the release-artifact cargo binary binds only to the cargo one --
/// never to the npm package sharing its bare name.
#[test]
fn matrix_same_bare_name_across_ecosystems_keys_by_qualified_id_and_binds_artifact_to_cargo_only() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_same_name_cross_ecosystem_fixture(root);
    let (cargo_id, npm_id) = same_name_cross_ecosystem_ids();
    let graph = same_name_cross_ecosystem_graph(cargo_id, npm_id);

    let cfg = callisto_graph::config::load(root).unwrap();
    let runner = DummyRunner;
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::new(),
        git: OnceCell::new(),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let report = matrix(&ws, &MatrixOptions::default()).expect("matrix should succeed");

    let keys: Vec<&String> = report.platform_targets.keys().collect();
    assert!(
        report.platform_targets.contains_key("cargo/shared"),
        "missing cargo/shared: {keys:?}"
    );
    assert!(
        report.platform_targets.contains_key("npm/shared"),
        "missing npm/shared: {keys:?}"
    );
    assert!(
        !report.platform_targets.contains_key("shared"),
        "bare `shared` key must not appear once both packages are qualified: {keys:?}"
    );

    assert_eq!(
        report.platform_targets["cargo/shared"].kind,
        callisto_model::PlatformTargetKind::Cargo
    );
    assert_eq!(report.platform_targets["cargo/shared"].targets.len(), 1);
    assert_eq!(
        report.platform_targets["cargo/shared"].targets[0].artifact_name, "shared.tar.gz",
        "the release artifact must bind to the cargo package, not the same-named npm one"
    );

    assert_eq!(
        report.platform_targets["npm/shared"].kind,
        callisto_model::PlatformTargetKind::Napi
    );
}

/// A bare `--package shared` is ambiguous between the two ecosystems and
/// must error listing both qualified candidates, never silently pick one.
#[test]
fn matrix_package_filter_bare_name_ambiguous_across_ecosystems_lists_both_candidates() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_same_name_cross_ecosystem_fixture(root);
    let (cargo_id, npm_id) = same_name_cross_ecosystem_ids();
    let graph = same_name_cross_ecosystem_graph(cargo_id.clone(), npm_id.clone());

    let cfg = callisto_graph::config::load(root).unwrap();
    let runner = DummyRunner;
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::new(),
        git: OnceCell::new(),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let opts = MatrixOptions {
        package: Some("shared".to_string()),
        ..Default::default()
    };
    let err = matrix(&ws, &opts).unwrap_err();
    match err {
        callisto_graph::GraphError::AmbiguousName { name, candidates } => {
            assert_eq!(name, "shared");
            assert_eq!(candidates.len(), 2, "{candidates:?}");
            assert!(candidates.contains(&cargo_id), "{candidates:?}");
            assert!(candidates.contains(&npm_id), "{candidates:?}");
        }
        other => panic!("expected AmbiguousName, got {other:?}"),
    }
}

/// A qualified `--package cargo/shared` resolves unambiguously to the cargo
/// package only, even though `npm/shared` shares its bare name.
#[test]
fn matrix_package_filter_qualified_name_selects_only_that_ecosystem() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_same_name_cross_ecosystem_fixture(root);
    let (cargo_id, npm_id) = same_name_cross_ecosystem_ids();
    let graph = same_name_cross_ecosystem_graph(cargo_id, npm_id);

    let cfg = callisto_graph::config::load(root).unwrap();
    let runner = DummyRunner;
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::new(),
        git: OnceCell::new(),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let opts = MatrixOptions {
        package: Some("cargo/shared".to_string()),
        ..Default::default()
    };
    let report = matrix(&ws, &opts).expect("qualified --package must resolve unambiguously");

    assert!(report.platform_targets.contains_key("cargo/shared"));
    assert!(!report.platform_targets.contains_key("npm/shared"));
    assert!(!report.runtime_versions.contains_key("npm/shared"));
}
