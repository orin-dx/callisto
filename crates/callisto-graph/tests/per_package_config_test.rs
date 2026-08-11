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

/// AC-1: A prefixed [[package]] rule (e.g. `cargo/my-crate`) must win over a bare
/// rule (e.g. `my-crate`) even when the bare rule appears EARLIER in callisto.toml.
///
/// Scenario: bare rule declares `release-trigger = "auto"` and appears first; prefixed
/// rule declares `changelog = "RELEASES.md"` and appears second.
/// After the fix, the prefixed rule wins for all fields:
///   - changelog ends with RELEASES.md   (prefixed rule's explicit setting)
///   - release_trigger is Changeset       (prefixed rule has no override → default)
///
/// The bare rule's `auto` trigger must NOT be applied.
#[test]
fn prefixed_rule_wins_over_bare_rule_regardless_of_declaration_order() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_cargo_workspace(root, "my-crate");

    fs::write(
        root.join("callisto.toml"),
        r#"
[[package]]
match = "my-crate"
release-trigger = "auto"

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

    let changelog = pkg.changelog.as_ref().expect("changelog should be set");
    assert!(
        changelog.ends_with("RELEASES.md"),
        "prefixed rule must win: expected changelog ending in RELEASES.md, got {changelog:?}",
    );
    assert_eq!(
        pkg.release_trigger,
        ReleaseTrigger::Changeset,
        "bare rule's release-trigger must NOT be applied when prefixed rule matches; \
         expected Changeset (prefixed rule default), got {:?}",
        pkg.release_trigger,
    );
}
/// AC-1b: A prefixed [[package]] rule wins by name alone — no ecosystem check
/// is performed against the package's actual manifests. Here, `npm/cross-eco`
/// is prefixed for npm but the package is Cargo-only; the prefixed rule still wins.
#[test]
fn prefixed_rule_wins_by_name_alone_no_ecosystem_check() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_cargo_workspace(root, "cross-eco");
    // bare rule FIRST, prefixed npm rule SECOND — prefixed must win regardless
    fs::write(
        root.join("callisto.toml"),
        r#"
[[package]]
match = "cross-eco"
release-trigger = "auto"

[[package]]
match = "npm/cross-eco"
changelog = "RELEASES.md"
"#,
    )
    .unwrap();
    let runner = NoopRunner;
    let ws = load_workspace(root, &runner);
    let pkg = ws
        .graph
        .packages()
        .find(|p| p.id.matches(&PackageId::parse("cross-eco").unwrap()))
        .expect("cross-eco should be discovered");
    let changelog = pkg.changelog.as_ref().expect("changelog should be set");
    assert!(
        changelog.ends_with("RELEASES.md"),
        "prefixed rule must win regardless of ecosystem match (AC-1b): got {changelog:?}",
    );
    assert_eq!(
        pkg.release_trigger,
        ReleaseTrigger::Changeset,
        "bare rule release-trigger must not apply (AC-1b); got {:?}",
        pkg.release_trigger,
    );
}

/// AC-2: When only Bare rules match a given package (non-matching Prefixed rules
/// for different packages may exist in cfg.packages), the first Bare rule wins.
#[test]
fn bare_rule_applies_when_no_prefixed_rule_matches_this_package() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_cargo_workspace(root, "target-crate");
    fs::write(
        root.join("callisto.toml"),
        r#"
[[package]]
match = "cargo/unrelated-crate"
changelog = "WRONG.md"

[[package]]
match = "target-crate"
release-trigger = "auto"
"#,
    )
    .unwrap();
    let runner = NoopRunner;
    let ws = load_workspace(root, &runner);
    let pkg = ws
        .graph
        .packages()
        .find(|p| p.id.matches(&PackageId::parse("target-crate").unwrap()))
        .expect("target-crate should be discovered");
    assert_eq!(
        pkg.release_trigger,
        ReleaseTrigger::Auto,
        "bare rule must apply when no prefixed rule matches this package (AC-2); got {:?}",
        pkg.release_trigger,
    );
}

/// AC-4a: When no [[package]] rule matches, the first matching [[package-set]]
/// rule in declaration order is applied (first-match-wins within the set tier).
#[test]
fn package_set_fallback_applies_first_match_wins() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_cargo_workspace(root, "solo");
    fs::write(
        root.join("callisto.toml"),
        r#"
[[package]]
match = "cargo/nomatch"
changelog = "WRONG.md"

[[package-set]]
match = "*"
release-trigger = "auto"

[[package-set]]
match = "solo*"
changelog = "ALSO-WRONG.md"
"#,
    )
    .unwrap();
    let runner = NoopRunner;
    let ws = load_workspace(root, &runner);
    let pkg = ws
        .graph
        .packages()
        .find(|p| p.id.matches(&PackageId::parse("solo").unwrap()))
        .expect("solo should be discovered");
    assert_eq!(
        pkg.release_trigger,
        ReleaseTrigger::Auto,
        "first [[package-set]] must win (AC-4a); got {:?}",
        pkg.release_trigger,
    );
    if let Some(cl) = &pkg.changelog {
        assert!(
            !cl.ends_with("ALSO-WRONG.md"),
            "second [[package-set]] must not win (AC-4a first-match-wins); got {cl:?}",
        );
    }
}

/// AC-4b: When neither [[package]] nor [[package-set]] rules match, the default
/// config is returned (no override).
#[test]
fn no_rule_match_returns_default_config() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_cargo_workspace(root, "orphan");
    fs::write(
        root.join("callisto.toml"),
        r#"
[[package]]
match = "cargo/nomatch"
changelog = "WRONG.md"
"#,
    )
    .unwrap();
    let runner = NoopRunner;
    let ws = load_workspace(root, &runner);
    let pkg = ws
        .graph
        .packages()
        .find(|p| p.id.matches(&PackageId::parse("orphan").unwrap()))
        .expect("orphan should be discovered");
    assert_eq!(
        pkg.release_trigger,
        ReleaseTrigger::Changeset,
        "unmatched package must have default release_trigger (AC-4b); got {:?}",
        pkg.release_trigger,
    );
    if let Some(cl) = &pkg.changelog {
        assert!(
            !cl.ends_with("WRONG.md"),
            "nomatch rule must not apply (AC-4b); got {cl:?}"
        );
    }
}
/// AC-3: When a package is matched only by Prefixed rules (no Bare rule in
/// cfg.packages matches it), the first Prefixed rule in declaration order wins.
/// Both `cargo/multi` and `npm/multi` are Prefixed and both match Bare("multi")
/// — Prefixed(E,x).matches(Bare(x)) is true for any E per PackageId::matches.
#[test]
fn first_prefixed_rule_wins_when_multiple_prefixed_rules_match() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_cargo_workspace(root, "multi");
    fs::write(
        root.join("callisto.toml"),
        r#"
[[package]]
match = "cargo/multi"
changelog = "FIRST.md"

[[package]]
match = "npm/multi"
changelog = "SECOND.md"
"#,
    )
    .unwrap();
    let runner = NoopRunner;
    let ws = load_workspace(root, &runner);
    let pkg = ws
        .graph
        .packages()
        .find(|p| p.id.matches(&PackageId::parse("multi").unwrap()))
        .expect("multi should be discovered");
    let changelog = pkg.changelog.as_ref().expect("changelog should be set");
    assert!(
        changelog.ends_with("FIRST.md"),
        "first Prefixed rule in declaration order must win (AC-3); got {changelog:?}",
    );
}
