/// Tests that [[package]] blocks in callisto.toml are applied to discovered packages.
///
/// These are regression tests for the bug where `ResolvedConfig.packages` was always
/// empty (`BTreeMap::new()`) because `load()` never populated it from `raw.package` entries.
use std::fs;
use std::path::Path;

use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::DependencyResolver;
use callisto_graph::Workspace;
use callisto_model::{CommandError, CommandOutput, CommandRunner, PackageId, ReleaseTrigger};

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

/// Write a minimal Cargo workspace with one member crate named `crate_name`
/// at `crates/<crate_name>/`.
fn write_cargo_workspace(root: &Path, crate_name: &str) {
    std::process::Command::new("git")
        .arg("init")
        .current_dir(root)
        .output()
        .expect("git init should run");

    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    )
    .unwrap();

    let crate_dir = root.join(format!("crates/{crate_name}"));
    fs::create_dir_all(&crate_dir).unwrap();
    fs::write(
        crate_dir.join("Cargo.toml"),
        format!("[package]\nname = \"{crate_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    )
    .unwrap();
}

fn load_workspace<'a>(root: &Path, runner: &'a NoopRunner) -> Workspace<'a, NoopRunner> {
    let locator = IgnoreWalkLocator::new(root);
    Workspace::load(root.to_path_buf(), &locator, runner).expect("workspace should load")
}

/// A [[package]] rule with `changelog = "RELEASES.md"` must override the
/// default `CHANGELOG.md` path for the matched package.
#[test]
fn per_package_changelog_path_override_is_applied() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_cargo_workspace(root, "my-crate");

    fs::write(
        root.join("callisto.toml"),
        r#"
[[package]]
match = "cargo/my-crate"
changelog = "RELEASES.md"
"#,
    )
    .unwrap();

    let runner = NoopRunner;
    let ws = load_workspace(root, &runner);

    let pkg = ws
        .graph
        .packages()
        .find(|p| p.id.matches(&PackageId::parse("my-crate").unwrap()))
        .expect("package my-crate should be discovered");

    // The override says RELEASES.md — the changelog should be relative
    // to the package directory (crates/my-crate/RELEASES.md), not the default
    // crates/my-crate/CHANGELOG.md.
    let changelog = pkg.changelog.as_ref().expect("changelog should be set");
    assert!(
        changelog.ends_with("RELEASES.md"),
        "expected changelog path ending in RELEASES.md, got {changelog:?}",
    );
    assert!(
        !changelog.ends_with("CHANGELOG.md"),
        "changelog path should not be the default CHANGELOG.md, got {changelog:?}",
    );
}

/// A [[package]] rule with `release-trigger = "auto"` must override the
/// default `ReleaseTrigger::Changeset` for the matched package.
#[test]
fn per_package_release_trigger_auto_is_applied() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_cargo_workspace(root, "my-crate");

    fs::write(
        root.join("callisto.toml"),
        r#"
[[package]]
match = "cargo/my-crate"
release-trigger = "auto"
"#,
    )
    .unwrap();

    let runner = NoopRunner;
    let ws = load_workspace(root, &runner);

    let pkg = ws
        .graph
        .packages()
        .find(|p| p.id.matches(&PackageId::parse("my-crate").unwrap()))
        .expect("package my-crate should be discovered");

    assert_eq!(
        pkg.release_trigger,
        ReleaseTrigger::Auto,
        "expected ReleaseTrigger::Auto from [[package]] config, got {:?}",
        pkg.release_trigger
    );
}

/// A [[package]] rule using a bare name (without ecosystem prefix) must match
/// a package discovered with that name.
#[test]
fn per_package_bare_name_match_works() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_cargo_workspace(root, "my-lib");

    fs::write(
        root.join("callisto.toml"),
        r#"
[[package]]
match = "my-lib"
release-trigger = "auto"
"#,
    )
    .unwrap();

    let runner = NoopRunner;
    let ws = load_workspace(root, &runner);

    let pkg = ws
        .graph
        .packages()
        .find(|p| p.id.matches(&PackageId::parse("my-lib").unwrap()))
        .expect("package my-lib should be discovered");

    assert_eq!(
        pkg.release_trigger,
        ReleaseTrigger::Auto,
        "bare match should apply config; got {:?}",
        pkg.release_trigger
    );
}

/// Packages NOT matched by any [[package]] rule must keep their defaults.
#[test]
fn unmatched_package_keeps_defaults() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_cargo_workspace(root, "other-crate");

    fs::write(
        root.join("callisto.toml"),
        r#"
[[package]]
match = "cargo/some-other-crate"
release-trigger = "auto"
"#,
    )
    .unwrap();

    let runner = NoopRunner;
    let ws = load_workspace(root, &runner);

    let pkg = ws
        .graph
        .packages()
        .find(|p| p.id.matches(&PackageId::parse("other-crate").unwrap()))
        .expect("package other-crate should be discovered");

    assert_eq!(
        pkg.release_trigger,
        ReleaseTrigger::Changeset,
        "unmatched package should keep Changeset default; got {:?}",
        pkg.release_trigger
    );

    let changelog = pkg.changelog.as_ref().expect("changelog should be set");
    assert!(
        changelog.ends_with("CHANGELOG.md"),
        "unmatched package should keep CHANGELOG.md default; got {changelog:?}",
    );
}
