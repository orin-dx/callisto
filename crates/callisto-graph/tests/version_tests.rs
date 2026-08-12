use callisto_graph::commands::{plan_version, VersionOptions};
use callisto_graph::infer::NoInference;
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::Workspace;
use callisto_model::{CommandError, CommandOutput, CommandRunner, PackageId, Severity};
use std::fs;
use std::path::Path;

struct NoopRunner;

impl CommandRunner for NoopRunner {
    fn run(
        &self,
        _program: &str,
        _args: &[&str],
        _cwd: &Path,
    ) -> Result<CommandOutput, CommandError> {
        Ok(CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

fn git_init_with_commit(root: &Path) {
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Test"],
    ] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(root)
            .output()
            .expect("git command should run");
    }
    fs::write(root.join(".gitkeep"), "").unwrap();
    for args in [vec!["add", "."], vec!["commit", "-q", "-m", "init"]] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(root)
            .output()
            .expect("git command should run");
    }
}

/// Verifies that plan_version produces correct bumps for all packages in a
/// cascade chain: pkg-core receives a Major bump from a changeset; pkg-app
/// depends on pkg-core via ^1.0.0 and should receive a cascade bump.
///
/// This test is a correctness guard for the O(N²) → O(N) refactor:
///   PERF-006: pre-built pkg_map in the plan_version severity loop.
///   PERF-008: HashSet-based dedup in the pre-release changeset update path.
#[test]
fn test_plan_version_produces_correct_bumps_in_cascade() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    // pkg-core: the library that receives a breaking-change bump directly.
    fs::create_dir_all(root.join("pkg-core")).unwrap();
    fs::write(
        root.join("pkg-core/Cargo.toml"),
        "[package]\nname = \"pkg-core\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    // pkg-app: depends on pkg-core, will receive a Patch cascade bump after
    // pkg-core bumps to 2.0.0 (out of range for ^1.0.0).
    fs::create_dir_all(root.join("pkg-app")).unwrap();
    fs::write(
        root.join("pkg-app/Cargo.toml"),
        "[package]\nname = \"pkg-app\"\nversion = \"1.0.0\"\n\n[dependencies]\npkg-core = \"1.0.0\"\n",
    )
    .unwrap();

    // Changeset: major bump on pkg-core only.
    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(
        root.join(".changeset/breaking-api.md"),
        "---\n\"pkg-core\": major\n---\n\nBreaking API change.\n",
    )
    .unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner)
        .expect("workspace should load from temp dir");

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        strict_graph: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version should succeed");

    let pkg_core = PackageId::parse("pkg-core").unwrap();
    let pkg_app = PackageId::parse("pkg-app").unwrap();

    let bump_for = |id: &PackageId| plan.bumps.iter().find(|b| &b.package == id);

    let core_bump = bump_for(&pkg_core).expect("pkg-core should have a planned bump");
    assert_eq!(
        core_bump.severity,
        Severity::Major,
        "pkg-core should receive a Major bump from the changeset"
    );

    let app_bump =
        bump_for(&pkg_app).expect("pkg-app should receive a cascade bump from pkg-core's major");
    assert!(
        app_bump.severity != Severity::None,
        "pkg-app cascade bump should be non-None, got {:?}",
        app_bump.severity
    );
}

/// Two packages in different directories sharing the same name must NOT
/// silently discard one of them. Before the fix, `ManifestWalkResolver::build`
/// used `BTreeMap::insert` with the `PackageId` (name-only) as key; the
/// second crate with the same name silently replaced the first with no error
/// or diagnostic, making one crate permanently invisible to all operations.
///
/// After the fix, `Workspace::load` must return `Err(GraphError::DuplicatePackage)`.
#[test]
fn duplicate_package_name_is_rejected_with_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // Two directories, both with a Cargo.toml declaring the same package name.
    for dir in &["packages/a", "packages/b"] {
        let pkg_dir = root.join(dir);
        fs::create_dir_all(&pkg_dir).unwrap();
        fs::write(
            pkg_dir.join("Cargo.toml"),
            "[package]\nname = \"shared-core\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
    }
    git_init_with_commit(root);

    let runner = NoopRunner;
    let locator = callisto_graph::locate::IgnoreWalkLocator::new(root);
    let result = Workspace::load(root.to_path_buf(), &locator, &runner);

    match result {
        Ok(_) => panic!(
            "Workspace::load must return Err when two packages share the same name, but got Ok"
        ),
        Err(e) => assert!(
            matches!(e, callisto_graph::GraphError::DuplicatePackage { .. }),
            "expected GraphError::DuplicatePackage, got: {e:?}"
        ),
    }
}
