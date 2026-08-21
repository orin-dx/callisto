use callisto_graph::commands::{plan_version, VersionOptions};
use callisto_graph::infer::NoInference;
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::Workspace;
use callisto_graph::{apply_version_plan, ApplyOptions};
use callisto_model::{
    ApplyPermit, CommandError, CommandOutput, CommandRunner, PackageId, Severity,
};
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

fn tag(root: &Path, name: &str) {
    std::process::Command::new("git")
        .args(["-c", "tag.gpgSign=false", "tag", "-m", "release", name])
        .current_dir(root)
        .output()
        .expect("git must be installed");
}

fn commit_all(root: &Path, message: &str) {
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .output()
        .expect("git add");
    std::process::Command::new("git")
        .args(["commit", "-q", "-m", message])
        .current_dir(root)
        .output()
        .expect("git commit");
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

/// AC-006: the real, on-disk Cargo+npm co-located dual-identity scenario
/// from the originating bug report -- a Cargo crate that is also its own
/// napi-rs npm package (Case D: same directory, two manifests, one owning
/// Bare PackageId since there is no naming collision elsewhere) -- must
/// proceed through apply_version_plan to a successful completion for a
/// cross-ecosystem npm dependent whose spec needs rewriting, with the
/// dependent's package.json mutated to keep the correct, already-in-use
/// dependency key. Before the fix, this aborts with
/// Err(ManifestError::DependencyNotFound) because solve_cascade constructs
/// the wrong key ("my-native-lib", the Cargo-native name) against a
/// manifest that only has an "@scope/my-native-lib" entry.
#[test]
fn apply_version_plan_succeeds_for_dual_identity_cross_ecosystem_rewrite_ac006() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    fs::create_dir_all(root.join("crates/my-native-lib")).unwrap();
    fs::write(
        root.join("crates/my-native-lib/Cargo.toml"),
        "[package]\nname = \"my-native-lib\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        root.join("crates/my-native-lib/package.json"),
        r#"{"name":"@scope/my-native-lib","version":"1.0.0"}"#,
    )
    .unwrap();

    fs::create_dir_all(root.join("packages/dep-app")).unwrap();
    fs::write(
        root.join("packages/dep-app/package.json"),
        r#"{"name":"dep-app","version":"1.0.0","dependencies":{"@scope/my-native-lib":"^1.0.0"}}"#,
    )
    .unwrap();

    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(
        root.join(".changeset/breaking-native-api.md"),
        "---\n\"my-native-lib\": major\n---\n\nBreaking native API change.\n",
    )
    .unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner)
        .expect("workspace with Case D dual-identity package should load");

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        strict_graph: false,
        allow_empty_changesets: true,
    };
    let plan = plan_version(&ws, &inference, &opts).expect("plan_version should succeed");

    let permit = ApplyPermit::force_for_tests();
    let apply_opts = ApplyOptions::default();
    let outcome = apply_version_plan(root, &plan, &runner, &apply_opts, &permit);

    assert!(
        outcome.is_ok(),
        "apply_version_plan must succeed for the dual-identity cross-ecosystem rewrite (AC-006); got: {:?}",
        outcome.err()
    );

    let dep_app_manifest = fs::read_to_string(root.join("packages/dep-app/package.json")).unwrap();
    assert!(
        dep_app_manifest.contains("\"@scope/my-native-lib\""),
        "dep-app's package.json must retain the ecosystem-native dependency key \"@scope/my-native-lib\"; got:\n{dep_app_manifest}"
    );
    assert!(
        !dep_app_manifest.contains("\"^1.0.0\""),
        "dep-app's dependency spec on my-native-lib must have been rewritten away from the now out-of-range \"^1.0.0\"; got:\n{dep_app_manifest}"
    );
}

/// A fixed group with two members, each named by its OWN, separate changeset
/// (pkg-a: minor via one changeset, pkg-b: minor via a different changeset --
/// mirroring a real multi-package release PR) must converge on exactly ONE
/// group-aligned bump matching the max severity across the whole group, not
/// a compounded/sequential bump. Both members start at 1.0.0; the expected
/// target is 1.1.0 (one minor step) for BOTH -- never 1.2.0 (which would
/// indicate the two changesets' minor severities were incorrectly applied
/// as two separate bumps instead of being unioned to one).
#[test]
fn test_fixed_group_two_changesets_converge_on_single_bump_not_compounded() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    fs::create_dir_all(root.join("pkg-a")).unwrap();
    fs::write(
        root.join("pkg-a/Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("pkg-b")).unwrap();
    fs::write(
        root.join("pkg-b/Cargo.toml"),
        "[package]\nname = \"pkg-b\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"ab-fixed\"\nmembers = [\"pkg-a\", \"pkg-b\"]\n",
    )
    .unwrap();

    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(
        root.join(".changeset/a-minor.md"),
        "---\n\"pkg-a\": minor\n---\n\nFeature in pkg-a.\n",
    )
    .unwrap();
    fs::write(
        root.join(".changeset/b-minor.md"),
        "---\n\"pkg-b\": minor\n---\n\nFeature in pkg-b.\n",
    )
    .unwrap();

    commit_all(root, "add packages");
    tag(root, "pkg-a@1.0.0");
    tag(root, "pkg-b@1.0.0");

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

    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();
    let bump_for = |id: &PackageId| plan.bumps.iter().find(|b| &b.package == id);

    let a_bump = bump_for(&pkg_a).expect("pkg-a should have a planned bump");
    let b_bump = bump_for(&pkg_b).expect("pkg-b should have a planned bump");

    assert_eq!(
        a_bump.to.render(),
        "1.1.0",
        "pkg-a must land on a single minor step (1.1.0), not a compounded bump; got: {}",
        a_bump.to.render()
    );
    assert_eq!(
        b_bump.to.render(),
        "1.1.0",
        "pkg-b must converge on the SAME single-step target as pkg-a (1.1.0), not its own \
         independently-bumped or compounded value; got: {}",
        b_bump.to.render()
    );
}
