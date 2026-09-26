mod fixtures;
use callisto_model::PackageId;
use fixtures::GraphBuilder;
use std::cell::OnceCell;

/// Runs `git` for test-fixture setup.
fn run_git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git should run");
    assert!(status.success(), "git {args:?} failed");
}

/// Initializes a real Git repo at `dir` with one commit and returns the full 40-char HEAD sha.
fn init_git_repo_with_commit(dir: &std::path::Path) -> String {
    callisto_fixtures::git::init_repo(dir);
    run_git(dir, &["add", "."]);
    run_git(dir, &["commit", "-q", "-m", "init"]);

    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .expect("git rev-parse HEAD should run");
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn test_snapshot_version_template_placeholders() {
    use callisto_graph::commands::plan_snapshot;

    let runner = callisto_fixtures::git::GitRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    // `plan_snapshot` must resolve a real HEAD sha, so the fixture needs a
    // real Git repository with at least one commit, not just a bare temp dir.
    std::fs::write(root.join(".gitkeep"), "").unwrap();
    let head_sha = init_git_repo_with_commit(root);
    let expected_sha7 = &head_sha[..7];

    let cfg = callisto_graph::config::load(&root.join("callisto.toml")).unwrap();
    let graph = GraphBuilder::new().build().unwrap();
    let git = callisto_model::vcs::GitAccess::new(root, &runner);
    let tags = callisto_graph::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        git: OnceCell::from(git),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let (_plan, report) = plan_snapshot(&ws, "canary").unwrap();
    assert_eq!(report.schema_version, callisto_model::SCHEMA_VERSION);
    assert_eq!(report.snapshot_tag, format!("0.0.0-canary-{expected_sha7}"));
}

/// The snapshot version is
/// exactly `0.0.0-{tag}-{sha7}` — base literally `0.0.0` (never the package's own version),
/// hyphen-joined (never dot-joined), and **identical for every package in the workspace**.
/// This is what makes a snapshot unpublishable-over-a-real-release: every genuine release
/// version sorts above `0.0.0-...` in SemVer precedence.
#[test]
fn test_snapshot_version_format_matches_spec() {
    use callisto_graph::commands::plan_snapshot;

    let runner = callisto_fixtures::git::GitRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    // Two packages with distinct, non-trivial real versions: if the implementation
    // (bug) bases the snapshot version on the package's own version, these two
    // packages will disagree; per spec, they must produce the identical string.
    let pkg_a_dir = root.join("pkg-a");
    std::fs::create_dir_all(&pkg_a_dir).unwrap();
    std::fs::write(
        pkg_a_dir.join("Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.4.2\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let pkg_b_dir = root.join("pkg-b");
    std::fs::create_dir_all(&pkg_b_dir).unwrap();
    std::fs::write(
        pkg_b_dir.join("Cargo.toml"),
        "[package]\nname = \"pkg-b\"\nversion = \"2.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let head_sha = init_git_repo_with_commit(root);
    let expected_sha7 = &head_sha[..7];
    let expected_version = format!("0.0.0-canary-{expected_sha7}");

    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();
    let graph = GraphBuilder::new()
        .package(pkg_a.clone(), |p| p)
        .package(pkg_b.clone(), |p| p)
        .build()
        .unwrap();

    let cfg = callisto_graph::config::load(&root.join("callisto.toml")).unwrap();
    let git = callisto_model::vcs::GitAccess::new(root, &runner);
    let tags = callisto_graph::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        git: OnceCell::from(git),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let (plan, report) = plan_snapshot(&ws, "canary").expect("plan_snapshot should succeed against a real repo");

    assert_eq!(
        report.snapshot_tag, expected_version,
        "snapshot_tag must be exactly `0.0.0-{{tag}}-{{sha7}}`"
    );

    assert_eq!(plan.bumps.len(), 2, "expected one planned bump per package");
    for bump in &plan.bumps {
        assert_eq!(
            bump.to.render(),
            expected_version.as_str(),
            "package `{}` must receive the identical workspace-wide snapshot version, \
             not a version derived from its own current version",
            bump.package.display_name()
        );
    }

    assert_eq!(report.bumps.len(), 2);
    for bump in &report.bumps {
        assert_eq!(bump.to.render(), expected_version.as_str());
    }
}

/// The snapshot sha is `CommitSha::short()` of a resolved HEAD.
/// When HEAD cannot be resolved (no repo, no commits, etc.), `plan_snapshot` must return a
/// real, surfaced error — not silently substitute a fake `0000000` placeholder sha, which
/// would let snapshots from unrelated runs collide on the same tag.
#[test]
fn test_snapshot_sha_resolution_failure_is_surfaced_error() {
    use callisto_graph::commands::plan_snapshot;

    let runner = callisto_fixtures::git::GitRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    // Deliberately no `git init`: the workspace root is not part of any Git repository,
    // so HEAD sha resolution must fail.
    let cfg = callisto_graph::config::load(&ws_dir.path().join("callisto.toml")).unwrap();
    let graph = GraphBuilder::new().build().unwrap();
    let git = callisto_model::vcs::GitAccess::new(ws_dir.path(), &runner);
    let tags = callisto_graph::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: ws_dir.path().to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        git: OnceCell::from(git),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let result = plan_snapshot(&ws, "canary");

    assert!(
        result.is_err(),
        "plan_snapshot must surface a real error when HEAD sha cannot be resolved, \
         not silently succeed with a fake `0000000` placeholder sha"
    );
    assert!(
        matches!(result, Err(callisto_graph::GraphError::Vcs(_))),
        "expected a GraphError::Vcs from failed sha discovery, got: {result:?}"
    );
}

/// `plan_snapshot` resolves HEAD with `git rev-parse HEAD` through the workspace's runner.
#[test]
fn test_snapshot_resolves_head_sha_through_the_command_runner() {
    use callisto_graph::commands::plan_snapshot;

    struct FakeHeadShaRunner(String);
    impl callisto_model::CommandRunner for FakeHeadShaRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            assert_eq!(program, "git");
            match args {
                ["rev-parse", "HEAD"] => Ok(callisto_model::CommandOutput {
                    exit_code: Some(0),
                    stdout: format!("{}\n", self.0),
                    stderr: String::new(),
                }),
                ["tag", "--list"] => Ok(callisto_model::CommandOutput {
                    exit_code: Some(0),
                    stdout: String::new(),
                    stderr: String::new(),
                }),
                other => panic!("unexpected git invocation: {other:?}"),
            }
        }
    }

    let head_sha = "b".repeat(40);
    let runner = FakeHeadShaRunner(head_sha.clone());
    let ws_dir = tempfile::tempdir().unwrap();

    let cfg = callisto_graph::config::load(&ws_dir.path().join("callisto.toml")).unwrap();
    let graph = GraphBuilder::new().build().unwrap();
    let git = callisto_model::vcs::GitAccess::new(ws_dir.path(), &runner);
    let tags = callisto_graph::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: ws_dir.path().to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        git: OnceCell::from(git),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let (_, report) = plan_snapshot(&ws, "canary").expect("plan_snapshot must resolve HEAD through the runner");

    let expected_version = format!("0.0.0-canary-{}", &head_sha[..7]);
    assert_eq!(report.snapshot_tag, expected_version);
}
