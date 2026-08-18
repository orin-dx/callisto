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

/// AC-006: --package restricts BOTH platformTargets AND runtimeVersions to
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

/// AC-007: an unknown --package name is a hard error naming the package,
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
    };
    let err = matrix(&ws, &opts).unwrap_err();
    match err {
        callisto_graph::GraphError::UnknownPackage { id } => {
            assert_eq!(id.name(), "does-not-exist");
        }
        other => panic!("expected UnknownPackage, got {other:?}"),
    }
}
