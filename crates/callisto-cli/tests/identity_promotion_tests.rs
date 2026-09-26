//! Adding a same-named package in another ecosystem promotes the display id only; tags, changelog and pre.json keep working.
#[path = "common/release_harness.rs"]
mod release_harness;

use std::{fs, path::Path};

use release_harness::{callisto, git, DECISION_PATH};

const NPM_FOO_TAG_TEMPLATE: &str = "[[package]]\nmatch = \"npm/foo\"\ntag-template = \"npm-foo@{version}\"\n";

/// Cargo `foo` 1.0.0 in `crates/foo`, released as `foo@1.0.0`.
fn released_cargo_foo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    git(root, &["remote", "add", "origin", "https://github.com/example/foo.git"]);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/foo\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/foo/src")).unwrap();
    fs::write(
        root.join("crates/foo/Cargo.toml"),
        "[package]\nname = \"foo\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("crates/foo/src/lib.rs"), "\n").unwrap();
    fs::write(
        root.join("crates/foo/CHANGELOG.md"),
        "# foo\n\n## 1.0.0\n\n- First release.\n",
    )
    .unwrap();
    callisto_fixtures::scaffold_callisto(root);
    commit(root, "release foo 1.0.0");
    git(root, &["tag", "foo@1.0.0"]);
    dir
}

/// npm `foo` in `packages/foo`, with the tag template its name collision requires.
fn add_npm_foo(root: &Path) {
    fs::create_dir_all(root.join("packages/foo")).unwrap();
    fs::write(
        root.join("packages/foo/package.json"),
        "{\n  \"name\": \"foo\",\n  \"version\": \"3.0.0\"\n}\n",
    )
    .unwrap();
    let config = fs::read_to_string(root.join("callisto.toml")).unwrap();
    fs::write(root.join("callisto.toml"), format!("{config}\n{NPM_FOO_TAG_TEMPLATE}")).unwrap();
    commit(root, "add npm foo");
}

fn add_changeset(root: &Path, name: &str, selector: &str, bump: &str) {
    fs::write(
        root.join(format!(".changeset/{name}.md")),
        format!("---\n\"{selector}\": {bump}\n---\n\nChange {name}.\n"),
    )
    .unwrap();
    commit(root, &format!("changeset {name}"));
}

fn commit(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", message]);
}

fn run_ok(root: &Path, args: &[&str]) -> serde_json::Value {
    let out = callisto(root, args);
    assert!(
        out.status.success(),
        "callisto {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null)
}

fn cargo_foo_version(root: &Path) -> String {
    let manifest = fs::read_to_string(root.join("crates/foo/Cargo.toml")).unwrap();
    manifest
        .lines()
        .find_map(|line| line.strip_prefix("version = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap()
        .to_string()
}

fn status_package<'a>(status: &'a serde_json::Value, id: &str) -> Option<&'a serde_json::Value> {
    status["packages"].as_array()?.iter().find(|pkg| pkg["package"] == id)
}

#[test]
fn promotion_keeps_the_last_tag_and_the_changelog() {
    let dir = released_cargo_foo();
    let root = dir.path();
    add_changeset(root, "feature", "cargo/foo", "minor");
    add_npm_foo(root);

    let status = run_ok(root, &["status"]);
    let foo = status_package(&status, "cargo/foo").unwrap_or_else(|| panic!("no cargo/foo in status: {status}"));
    assert_eq!(foo["lastReleasedVersion"], "1.0.0", "status: {status}");

    run_ok(root, &["version", "--no-refresh-lockfiles"]);
    assert_eq!(cargo_foo_version(root), "1.1.0");
    let changelog = fs::read_to_string(root.join("crates/foo/CHANGELOG.md")).unwrap();
    assert!(
        changelog.contains("1.1.0") && changelog.contains("## 1.0.0"),
        "{changelog}"
    );
    assert!(!root.join("packages/foo/CHANGELOG.md").exists());
}

#[test]
fn same_named_packages_without_a_tag_template_fail_with_e101() {
    let dir = released_cargo_foo();
    let root = dir.path();
    fs::create_dir_all(root.join("packages/foo")).unwrap();
    fs::write(
        root.join("packages/foo/package.json"),
        "{\n  \"name\": \"foo\",\n  \"version\": \"3.0.0\"\n}\n",
    )
    .unwrap();
    commit(root, "add npm foo");
    add_changeset(root, "feature", "cargo/foo", "minor");

    let out = callisto(root, &["version", "--no-refresh-lockfiles"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "version must refuse shared default tags");
    assert!(stderr.contains("E101"), "{stderr}");
    assert!(stderr.contains("cargo/foo") && stderr.contains("npm/foo"), "{stderr}");
    assert_eq!(cargo_foo_version(root), "1.0.0");
}

#[test]
fn a_pre_cycle_started_before_promotion_keeps_its_initial_version() {
    let dir = released_cargo_foo();
    let root = dir.path();
    run_ok(root, &["pre", "enter", "beta"]);
    commit(root, "enter pre");
    add_changeset(root, "first", "cargo/foo", "minor");
    run_ok(root, &["version", "--no-refresh-lockfiles"]);
    assert_eq!(cargo_foo_version(root), "1.1.0-beta.0");
    commit(root, "version beta.0");

    add_npm_foo(root);
    add_changeset(root, "second", "cargo/foo", "patch");
    run_ok(root, &["version", "--no-refresh-lockfiles"]);
    assert_eq!(cargo_foo_version(root), "1.1.0-beta.1");
}

#[test]
fn a_decision_written_by_version_verifies_in_release_plan_after_promotion() {
    let dir = released_cargo_foo();
    let root = dir.path();
    add_npm_foo(root);
    add_changeset(root, "feature", "cargo/foo", "minor");
    run_ok(
        root,
        &["version", "--no-refresh-lockfiles", "--emit-decision", DECISION_PATH],
    );
    commit(root, "release");
    let release_commit = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "--detach", &release_commit]);
    let external = tempfile::tempdir().unwrap();
    let out = external.path().join("intent.json");
    run_ok(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            &release_commit,
            "--decision",
            DECISION_PATH,
            "--orchestration-revision",
            &release_commit,
            "--artifact-repository",
            "example/foo",
            "--out",
            out.to_str().unwrap(),
        ],
    );
    let intent: serde_json::Value = serde_json::from_slice(&fs::read(&out).unwrap()).unwrap();
    let tagged = intent["operations"].as_array().unwrap().iter().any(|op| {
        op["id"]["package"] == "cargo/foo" && op["id"]["version"] == "1.1.0" && op["id"]["role"]["kind"] == "tag"
    });
    assert!(tagged, "{intent}");
}

#[test]
fn display_id_is_bare_when_unique_and_qualified_when_promoted() {
    let dir = released_cargo_foo();
    let root = dir.path();

    let status = run_ok(root, &["status"]);
    assert!(status_package(&status, "foo").is_some(), "status: {status}");
    run_ok(root, &["add", "--package", "cargo/foo:patch", "--summary", "Before"]);

    add_npm_foo(root);
    let status = run_ok(root, &["status"]);
    assert!(status_package(&status, "cargo/foo").is_some(), "status: {status}");
    assert!(status_package(&status, "npm/foo").is_some(), "status: {status}");
    assert!(status_package(&status, "foo").is_none(), "status: {status}");
    run_ok(root, &["add", "--package", "cargo/foo:patch", "--summary", "After"]);

    let out = callisto(root, &["add", "--package", "foo:patch", "--summary", "Ambiguous"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E103") && stderr.contains("cargo/foo"), "{stderr}");
}
