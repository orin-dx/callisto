mod fixtures;
use callisto_model::{DepKind, DepSpec, PackageId, PublishTarget};
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

fn init_git_repo(dir: &std::path::Path) {
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git should run");
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test"]);
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "init"]);
}

/// Plan publish must emit Cargo crates in dependency-first (topological) order:
/// a dependency must appear before every package that depends on it so that
/// downstream consumers can reference the already-published version.
///
/// Graph: pkg-c (no deps) <- pkg-b <- pkg-a
/// Expected rust_crates order: [pkg-c, pkg-b, pkg-a]
#[test]
fn test_publish_plan_uses_correct_topological_order() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let runner = DummyRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    // Write Cargo.toml manifests for all three packages so base_versions() can read them.
    for name in &["pkg-a", "pkg-b", "pkg-c"] {
        let pkg_dir = root.join(name);
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
    }

    // A real git repo is needed so that tags() initialisation does not fail.
    // No tags are created, so every package will be considered a release candidate
    // (tag_match = false -> is_release = true).
    init_git_repo(root);

    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();
    let pkg_c = PackageId::parse("pkg-c").unwrap();

    // Build graph: pkg-a -> pkg-b -> pkg-c (pkg-c has no dependencies).
    let graph = GraphBuilder::new()
        .package(pkg_a.clone(), |p: PackageBuilder| {
            p.publish_to(vec![PublishTarget::CratesIo])
        })
        .package(pkg_b.clone(), |p: PackageBuilder| {
            p.publish_to(vec![PublishTarget::CratesIo])
        })
        .package(pkg_c.clone(), |p: PackageBuilder| {
            p.publish_to(vec![PublishTarget::CratesIo])
        })
        .edge(
            pkg_a.clone(),
            pkg_b.clone(),
            DepKind::Runtime,
            DepSpec::Opaque("1.0.0".to_string()),
        )
        .edge(
            pkg_b.clone(),
            pkg_c.clone(),
            DepKind::Runtime,
            DepSpec::Opaque("1.0.0".to_string()),
        )
        .build()
        .unwrap();

    let cfg = callisto_graph::config::load(&root.join("callisto.toml")).unwrap();
    let tags = callisto_graph::tags::TagIndex::build(&runner, root, &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        runner: &runner,
        manifest_cache: Default::default(),
    };

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish should succeed");

    // All three packages should be in the plan (no tags => all are release candidates).
    assert_eq!(
        plan.rust_crates.len(),
        3,
        "expected all three packages in the plan; got: {:?}",
        plan.rust_crates.iter().map(|c| &c.name).collect::<Vec<_>>()
    );

    let names: Vec<&str> = plan.rust_crates.iter().map(|c| c.name.as_str()).collect();

    // pkg-c must appear before pkg-b (pkg-b depends on pkg-c).
    let pos_c = names
        .iter()
        .position(|&n| n == "pkg-c")
        .expect("pkg-c missing from plan");
    let pos_b = names
        .iter()
        .position(|&n| n == "pkg-b")
        .expect("pkg-b missing from plan");
    let pos_a = names
        .iter()
        .position(|&n| n == "pkg-a")
        .expect("pkg-a missing from plan");

    assert!(
        pos_c < pos_b,
        "pkg-c must precede pkg-b in publish plan (dependency first); order: {names:?}"
    );
    assert!(
        pos_b < pos_a,
        "pkg-b must precede pkg-a in publish plan (dependency first); order: {names:?}"
    );
}
