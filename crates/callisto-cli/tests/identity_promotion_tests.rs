//! Adding a same-named package in another ecosystem promotes the display id only; tags, changelog and pre.json keep working.
#[path = "common/release_harness.rs"]
mod release_harness;

use std::{fs, path::Path};

use release_harness::{callisto, git, DECISION_PATH};

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

/// npm `foo` in `packages/foo`, with no tag configuration.
fn add_npm_foo(root: &Path) {
    fs::create_dir_all(root.join("packages/foo")).unwrap();
    fs::write(
        root.join("packages/foo/package.json"),
        "{\n  \"name\": \"foo\",\n  \"version\": \"3.0.0\"\n}\n",
    )
    .unwrap();
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

fn promoted_dual_npm_source_workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/source\", \"packages/app\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/source/src")).unwrap();
    fs::write(
        root.join("crates/source/Cargo.toml"),
        "[package]\nname = \"source\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("crates/source/src/lib.rs"), "\n").unwrap();
    add_cargo_package(root, "packages/app", "app", "");
    fs::write(
        root.join("crates/source/package.json"),
        "{\n  \"name\": \"@native/source\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("packages/source")).unwrap();
    fs::write(
        root.join("packages/source/package.json"),
        "{\n  \"name\": \"source\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("packages/app/src")).unwrap();
    fs::write(root.join("packages/app/src/lib.rs"), "\n").unwrap();
    fs::write(
        root.join("packages/app/package.json"),
        "{\n  \"name\": \"@native/app\",\n  \"version\": \"1.0.0\",\n  \"dependencies\": {\"@native/source\": \"^1.0.0\"},\n  \"optionalDependencies\": {\"@native/source\": \"^1.0.0\"},\n  \"peerDependencies\": {\"@native/source\": \"^1.0.0\"},\n  \"devDependencies\": {\"@native/source\": \"^1.0.0\"}\n}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("packages/app-sibling")).unwrap();
    fs::write(
        root.join("packages/app-sibling/package.json"),
        "{\n  \"name\": \"app\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .unwrap();
    callisto_fixtures::scaffold_callisto(root);
    commit(root, "initial promoted dual npm source");
    dir
}

fn cargo_workspace(root: &Path, members: &[&str]) {
    callisto_fixtures::git::init_repo(root);
    let member_list = members
        .iter()
        .map(|member| format!("\"{member}\""))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nmembers = [{member_list}]\nresolver = \"2\"\n"),
    )
    .unwrap();
    callisto_fixtures::scaffold_callisto(root);
}

fn add_cargo_package(root: &Path, relative: &str, name: &str, extra: &str) {
    fs::create_dir_all(root.join(relative).join("src")).unwrap();
    fs::write(
        root.join(relative).join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n{extra}"),
    )
    .unwrap();
    fs::write(root.join(relative).join("src/lib.rs"), "\n").unwrap();
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
fn same_named_packages_need_no_tag_configuration() {
    let dir = released_cargo_foo();
    let root = dir.path();
    add_npm_foo(root);
    add_changeset(root, "feature", "cargo/foo", "minor");

    let status = run_ok(root, &["status"]);
    let npm = status_package(&status, "npm/foo").unwrap_or_else(|| panic!("no npm/foo in status: {status}"));
    assert!(
        npm["lastReleasedVersion"].is_null(),
        "npm/foo must not claim cargo foo's tags: {status}"
    );

    run_ok(root, &["version", "--no-refresh-lockfiles"]);
    assert_eq!(cargo_foo_version(root), "1.1.0");
}

#[test]
fn a_moved_directory_keeps_the_bare_tag_history() {
    let dir = released_cargo_foo();
    let root = dir.path();
    add_npm_foo(root);
    git(root, &["mv", "crates/foo", "crates/foo-core"]);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/foo-core\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    commit(root, "move cargo foo");

    let status = run_ok(root, &["status"]);
    let foo = status_package(&status, "cargo/foo").unwrap_or_else(|| panic!("no cargo/foo in status: {status}"));
    assert_eq!(foo["lastReleasedVersion"], "1.0.0", "status: {status}");
    let npm = status_package(&status, "npm/foo").unwrap_or_else(|| panic!("no npm/foo in status: {status}"));
    assert!(npm["lastReleasedVersion"].is_null(), "status: {status}");
}

#[test]
fn a_package_that_gains_a_primary_manifest_keeps_the_bare_tag_history() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    fs::create_dir_all(root.join("packages/foo")).unwrap();
    fs::write(
        root.join("packages/foo/package.json"),
        "{\n  \"name\": \"foo\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .unwrap();
    callisto_fixtures::scaffold_callisto(root);
    commit(root, "release npm foo 1.0.0");
    git(root, &["tag", "foo@1.0.0"]);
    fs::create_dir_all(root.join("py/foo")).unwrap();
    fs::write(
        root.join("py/foo/pyproject.toml"),
        "[project]\nname = \"foo\"\nversion = \"3.0.0\"\n",
    )
    .unwrap();
    commit(root, "add python foo");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"packages/foo\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("packages/foo/src")).unwrap();
    fs::write(root.join("packages/foo/src/lib.rs"), "\n").unwrap();
    fs::write(
        root.join("packages/foo/Cargo.toml"),
        "[package]\nname = \"foo\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    commit(root, "add cargo foo beside npm foo");

    let status = run_ok(root, &["status"]);
    let foo = status_package(&status, "cargo/foo").unwrap_or_else(|| panic!("no cargo/foo in status: {status}"));
    assert_eq!(foo["lastReleasedVersion"], "1.0.0", "status: {status}");
    let py = status_package(&status, "pypi/foo").unwrap_or_else(|| panic!("no pypi/foo in status: {status}"));
    assert!(py["lastReleasedVersion"].is_null(), "status: {status}");
}

#[test]
fn a_demoted_package_keeps_the_qualified_tag_it_made_while_promoted() {
    let dir = released_cargo_foo();
    let root = dir.path();
    add_npm_foo(root);
    fs::write(
        root.join("crates/foo/Cargo.toml"),
        "[package]\nname = \"foo\"\nversion = \"2.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    commit(root, "release cargo foo 2.0.0");
    git(root, &["tag", "cargo/foo@2.0.0"]);
    git(root, &["rm", "-r", "-q", "packages/foo"]);
    git(root, &["commit", "-m", "remove npm foo"]);

    let status = run_ok(root, &["status"]);
    let foo = status_package(&status, "cargo/foo").unwrap_or_else(|| panic!("no cargo/foo in status: {status}"));
    assert_eq!(foo["lastReleasedVersion"], "2.0.0", "status: {status}");
}

/// An empty workspace repository with Callisto scaffolded and no packages.
fn empty_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    callisto_fixtures::git::init_repo(dir.path());
    callisto_fixtures::scaffold_callisto(dir.path());
    dir
}

fn write_manifest(root: &Path, relative: &str, content: String) {
    fs::create_dir_all(root.join(relative).parent().unwrap()).unwrap();
    fs::write(root.join(relative), content).unwrap();
}

fn cargo_manifest(name: &str, version: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n")
}

fn npm_manifest(name: &str, version: &str) -> String {
    format!("{{\n  \"name\": \"{name}\",\n  \"version\": \"{version}\"\n}}\n")
}

fn pypi_manifest(name: &str, version: &str) -> String {
    format!("[project]\nname = \"{name}\"\nversion = \"{version}\"\n")
}

/// Cargo `foo` with a second npm manifest named `foo-js` in `crates/a`, and Python `foo` in `py/z`.
fn add_lookalike_owners(root: &Path) {
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/a\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/a/src")).unwrap();
    fs::write(root.join("crates/a/src/lib.rs"), "\n").unwrap();
    write_manifest(root, "crates/a/Cargo.toml", cargo_manifest("foo", "0.1.0"));
    write_manifest(root, "crates/a/package.json", npm_manifest("foo-js", "0.1.0"));
    write_manifest(root, "py/z/pyproject.toml", pypi_manifest("foo", "0.1.0"));
}

#[test]
fn a_deleted_packages_bare_tag_is_not_adopted_by_a_package_whose_second_manifest_has_another_name() {
    let dir = empty_repo();
    let root = dir.path();
    write_manifest(root, "packages/y/package.json", npm_manifest("foo", "5.0.0"));
    commit(root, "release npm foo");
    git(root, &["tag", "foo@5.0.0"]);
    git(root, &["rm", "-r", "-q", "packages/y"]);
    add_lookalike_owners(root);
    commit(root, "replace npm foo with lookalikes");

    let status = run_ok(root, &["status"]);
    for id in ["cargo/foo", "pypi/foo"] {
        let package = status_package(&status, id).unwrap_or_else(|| panic!("no {id} in status: {status}"));
        assert!(
            package["lastReleasedVersion"].is_null(),
            "{id} adopted the tag: {status}"
        );
    }
}

#[test]
fn a_package_keeps_its_history_beside_a_lookalike_with_a_second_manifest() {
    let dir = empty_repo();
    let root = dir.path();
    write_manifest(root, "packages/b/package.json", npm_manifest("foo", "1.0.0"));
    commit(root, "release npm foo");
    git(root, &["tag", "foo@1.0.0"]);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/a\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/a/src")).unwrap();
    fs::write(root.join("crates/a/src/lib.rs"), "\n").unwrap();
    write_manifest(root, "crates/a/Cargo.toml", cargo_manifest("foo", "1.0.0"));
    write_manifest(root, "crates/a/package.json", npm_manifest("foo-js", "1.0.0"));
    git(root, &["mv", "packages/b", "packages/b2"]);
    commit(root, "add the lookalike and move npm foo");

    let status = run_ok(root, &["status"]);
    let npm = status_package(&status, "npm/foo").unwrap_or_else(|| panic!("no npm/foo in status: {status}"));
    assert_eq!(npm["lastReleasedVersion"], "1.0.0", "status: {status}");
    let cargo = status_package(&status, "cargo/foo").unwrap_or_else(|| panic!("no cargo/foo in status: {status}"));
    assert!(cargo["lastReleasedVersion"].is_null(), "status: {status}");
}

#[test]
fn a_pre_release_baseline_of_a_deleted_package_is_not_adopted_by_a_lookalike() {
    let dir = empty_repo();
    let root = dir.path();
    write_manifest(root, "packages/y/package.json", npm_manifest("foo", "5.0.0"));
    fs::write(
        root.join(".changeset/pre.json"),
        "{\"mode\":\"pre\",\"tag\":\"beta\",\"initialVersions\":{\"foo\":\"5.0.0\"},\"changesets\":[]}\n",
    )
    .unwrap();
    commit(root, "enter pre with npm foo");
    git(root, &["rm", "-r", "-q", "packages/y"]);
    add_lookalike_owners(root);
    commit(root, "replace npm foo with lookalikes");

    let out = callisto(root, &["status"]);
    assert!(!out.status.success(), "the baseline must not be adopted");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("E245"), "{text}");
}

#[test]
fn a_bare_tag_that_points_at_a_blob_does_not_break_history_lookup() {
    let dir = released_cargo_foo();
    let root = dir.path();
    add_npm_foo(root);
    git(root, &["mv", "crates/foo", "crates/foo-core"]);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/foo-core\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    commit(root, "move cargo foo");
    let blob = git(root, &["rev-parse", "HEAD:Cargo.toml"]);
    git(root, &["tag", "foo@0.5.0", &blob]);

    let out = callisto(root, &["status"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let foo = status_package(&status, "cargo/foo").unwrap_or_else(|| panic!("no cargo/foo in status: {status}"));
    assert_eq!(foo["lastReleasedVersion"], "1.0.0", "status: {status}");
}

#[test]
fn a_bare_changeset_name_after_promotion_says_how_to_qualify_it() {
    let dir = released_cargo_foo();
    let root = dir.path();
    add_npm_foo(root);
    add_changeset(root, "feature", "foo", "minor");

    let out = callisto(root, &["version", "--no-refresh-lockfiles"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        stderr.contains("E103") && stderr.contains("`cargo/foo`") && stderr.contains("`npm/foo`"),
        "{stderr}"
    );
    assert!(stderr.contains("cargo/foo: minor"), "{stderr}");
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
fn a_report_names_a_package_qualified_whether_or_not_it_was_promoted() {
    let dir = released_cargo_foo();
    let root = dir.path();

    let status = run_ok(root, &["status"]);
    assert!(status_package(&status, "cargo/foo").is_some(), "status: {status}");
    run_ok(root, &["add", "--package", "cargo/foo:patch", "--summary", "Before"]);

    add_npm_foo(root);
    let status = run_ok(root, &["status"]);
    assert!(status_package(&status, "cargo/foo").is_some(), "status: {status}");
    assert!(status_package(&status, "npm/foo").is_some(), "status: {status}");
    assert!(status_package(&status, "foo").is_none(), "status: {status}");
    run_ok(root, &["add", "--package", "cargo/foo:patch", "--summary", "After"]);

    let out = callisto(root, &["add", "--package", "foo:patch", "--summary", "Bare"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E317") && stderr.contains("cargo/foo"), "{stderr}");
}

#[test]
fn version_rewrites_npm_dependency_names_for_promoted_dual_source() {
    let dir = promoted_dual_npm_source_workspace();
    let root = dir.path();
    add_changeset(root, "source", "cargo/source", "major");

    let status = run_ok(root, &["status"]);
    assert!(status_package(&status, "cargo/app").is_some(), "status: {status}");
    assert!(status_package(&status, "npm/app").is_some(), "status: {status}");

    run_ok(root, &["version", "--no-refresh-lockfiles"]);

    let dependent: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("packages/app/package.json")).unwrap()).unwrap();
    for section in [
        "dependencies",
        "optionalDependencies",
        "peerDependencies",
        "devDependencies",
    ] {
        assert_eq!(dependent[section]["@native/source"], "^2.0.0", "{section}: {dependent}");
        assert!(dependent[section].get("source").is_none(), "{section}: {dependent}");
    }
    let npm_owner: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("crates/source/package.json")).unwrap()).unwrap();
    assert_eq!(npm_owner["name"], "@native/source");
    assert_eq!(npm_owner["version"], "2.0.0");
    let npm_dependent: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("packages/app/package.json")).unwrap()).unwrap();
    assert_eq!(npm_dependent["name"], "@native/app");
}

#[test]
fn version_rewrites_a_renamed_cargo_dependency_without_losing_its_alias() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    cargo_workspace(root, &["crates/source", "crates/app"]);
    add_cargo_package(root, "crates/source", "native-source", "");
    add_cargo_package(
        root,
        "crates/app",
        "app",
        "\n[dependencies]\nsource-alias = { package = \"native-source\", path = \"../source\", version = \"^1.0.0\" }\n",
    );
    commit(root, "initial renamed cargo dependency");
    add_changeset(root, "source", "cargo/native-source", "major");

    run_ok(root, &["version", "--no-refresh-lockfiles"]);

    let manifest = fs::read_to_string(root.join("crates/app/Cargo.toml")).unwrap();
    assert!(
        manifest.contains("source-alias = { package = \"native-source\""),
        "{manifest}"
    );
    assert!(manifest.contains("version = \"^2.0.0\""), "{manifest}");
    assert!(!manifest.contains("native-source = {"), "{manifest}");
}

#[test]
fn version_rewrites_an_inherited_cargo_alias_in_the_workspace_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    cargo_workspace(root, &["crates/source", "crates/app"]);
    add_cargo_package(root, "crates/source", "native-source", "");
    add_cargo_package(
        root,
        "crates/app",
        "app",
        "\n[dependencies]\nsource-alias = { workspace = true }\n",
    );
    let workspace = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "{workspace}\n[workspace.dependencies]\nsource-alias = {{ package = \"native-source\", path = \"crates/source\", version = \"^1.0.0\" }}\n"
        ),
    )
    .unwrap();
    commit(root, "initial inherited cargo alias");
    add_changeset(root, "source", "cargo/native-source", "major");

    run_ok(root, &["version", "--no-refresh-lockfiles"]);

    let workspace = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(
        workspace.contains("source-alias = { package = \"native-source\""),
        "{workspace}"
    );
    assert!(workspace.contains("version = \"^2.0.0\""), "{workspace}");
    assert!(!workspace.contains("native-source = {"), "{workspace}");
    let member = fs::read_to_string(root.join("crates/app/Cargo.toml")).unwrap();
    assert!(member.contains("source-alias = { workspace = true }"), "{member}");
}

#[test]
fn a_package_renamed_into_a_shared_name_does_not_take_the_tag_of_the_package_that_owned_it() {
    let dir = empty_repo();
    let root = dir.path();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/old\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/old/src")).unwrap();
    fs::write(root.join("crates/old/src/lib.rs"), "\n").unwrap();
    write_manifest(root, "crates/old/Cargo.toml", cargo_manifest("foo", "1.0.0"));
    write_manifest(root, "packages/b/package.json", npm_manifest("bar", "1.0.0"));
    commit(root, "release cargo foo");
    git(root, &["tag", "foo@1.0.0"]);

    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/a\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    git(root, &["mv", "crates/old", "crates/a"]);
    write_manifest(root, "packages/b/package.json", npm_manifest("foo", "1.0.0"));
    commit(root, "move cargo foo and rename npm bar to foo");

    let status = run_ok(root, &["status"]);
    let cargo = status_package(&status, "cargo/foo").unwrap_or_else(|| panic!("no cargo/foo in status: {status}"));
    assert_eq!(cargo["lastReleasedVersion"], "1.0.0", "status: {status}");
    let npm = status_package(&status, "npm/foo").unwrap_or_else(|| panic!("no npm/foo in status: {status}"));
    assert!(npm["lastReleasedVersion"].is_null(), "status: {status}");
}

#[test]
fn a_bare_pre_release_baseline_is_not_adopted_by_a_package_that_only_carries_the_name_now() {
    let dir = empty_repo();
    let root = dir.path();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/foo\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/foo/src")).unwrap();
    fs::write(root.join("crates/foo/src/lib.rs"), "\n").unwrap();
    write_manifest(root, "crates/foo/Cargo.toml", cargo_manifest("foo", "1.0.0"));
    fs::write(
        root.join(".changeset/pre.json"),
        "{\"mode\":\"pre\",\"tag\":\"beta\",\"initialVersions\":{\"foo\":\"1.0.0\"},\"changesets\":[]}\n",
    )
    .unwrap();
    commit(root, "enter pre with cargo foo");

    write_manifest(root, "crates/foo/Cargo.toml", cargo_manifest("foo-core", "1.0.0"));
    write_manifest(root, "packages/foo/package.json", npm_manifest("foo", "3.0.0"));
    commit(root, "rename cargo foo and add an npm foo");
    add_changeset(root, "feature", "foo", "minor");

    run_ok(root, &["version", "--no-refresh-lockfiles"]);
    let manifest = fs::read_to_string(root.join("packages/foo/package.json")).unwrap();
    assert!(manifest.contains("\"version\": \"3.1.0-beta.0\""), "{manifest}");
}

/// npm `foo` released as `foo@<version>`, then a Cargo `foo` 0.1.0 added beside it.
fn released_npm_foo_with_a_cargo_lookalike(version: &str) -> tempfile::TempDir {
    let dir = empty_repo();
    let root = dir.path();
    write_manifest(root, "packages/foo/package.json", npm_manifest("foo", version));
    commit(root, "release npm foo");
    git(root, &["tag", &format!("foo@{version}")]);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/foo\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/foo/src")).unwrap();
    fs::write(root.join("crates/foo/src/lib.rs"), "\n").unwrap();
    write_manifest(root, "crates/foo/Cargo.toml", cargo_manifest("foo", "0.1.0"));
    commit(root, "add a cargo foo");
    dir
}

#[test]
fn a_package_that_gained_the_name_when_the_owner_was_renamed_takes_none_of_its_tags() {
    let dir = released_npm_foo_with_a_cargo_lookalike("1.0.0");
    let root = dir.path();
    write_manifest(root, "packages/foo/package.json", npm_manifest("bar", "1.0.0"));
    commit(root, "rename npm foo to bar");

    let status = run_ok(root, &["status"]);
    let foo = status_package(&status, "cargo/foo").unwrap_or_else(|| panic!("no cargo/foo in status: {status}"));
    assert!(foo["lastReleasedVersion"].is_null(), "foo adopted the tag: {status}");
    let diagnostics = status["diagnostics"].to_string();
    assert!(diagnostics.contains("tag-owned-by-another-package"), "{status}");
    assert!(diagnostics.contains("previous-tag-templates"), "{status}");
}

#[test]
fn a_package_that_gained_the_name_when_the_owner_was_removed_takes_none_of_its_tags() {
    let dir = released_npm_foo_with_a_cargo_lookalike("5.0.0");
    let root = dir.path();
    git(root, &["rm", "-r", "-q", "packages/foo"]);
    commit(root, "remove npm foo");

    let status = run_ok(root, &["status"]);
    let foo = status_package(&status, "cargo/foo").unwrap_or_else(|| panic!("no cargo/foo in status: {status}"));
    assert!(foo["lastReleasedVersion"].is_null(), "foo adopted the tag: {status}");
}

#[test]
fn a_package_whose_version_was_tagged_by_another_package_is_still_planned_for_release() {
    let dir = released_npm_foo_with_a_cargo_lookalike("0.1.0");
    let root = dir.path();
    write_manifest(root, "packages/foo/package.json", npm_manifest("bar", "0.1.0"));
    commit(root, "rename npm foo to bar");

    let out = callisto(root, &["--dry-run", "release"]);
    let plan = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && plan.contains("\"cargo/foo\""),
        "the cargo foo must be planned, not skipped as tagged: {plan}{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_bare_package_keeps_the_tags_it_made() {
    let dir = released_cargo_foo();
    let root = dir.path();
    let status = run_ok(root, &["status"]);
    let foo = status_package(&status, "cargo/foo").unwrap_or_else(|| panic!("no cargo/foo in status: {status}"));
    assert_eq!(foo["lastReleasedVersion"], "1.0.0", "status: {status}");
    assert!(!status["diagnostics"]
        .to_string()
        .contains("tag-owned-by-another-package"));
}

/// One bad entry (a typo, a removed package) makes the whole changeset wait: nothing from it applies, so
/// running `version` again and again never bumps the package it also names.
#[test]
fn a_changeset_with_an_unresolved_entry_applies_nothing_on_any_run() {
    let dir = empty_repo();
    let root = dir.path();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"core\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("core/src")).unwrap();
    fs::write(root.join("core/src/lib.rs"), "\n").unwrap();
    write_manifest(root, "core/Cargo.toml", cargo_manifest("core", "1.0.0"));
    fs::write(
        root.join(".changeset/typo.md"),
        "---\n\"cargo/core\": minor\n\"cargo/ghost\": patch\n---\n\nA feature.\n",
    )
    .unwrap();
    commit(root, "a changeset with a typo");

    for _ in 0..2 {
        let out = callisto(root, &["version", "--no-refresh-lockfiles"]);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(text.contains("none of its entries apply"), "{text}");
        let manifest = fs::read_to_string(root.join("core/Cargo.toml")).unwrap();
        assert!(manifest.contains("version = \"1.0.0\""), "{manifest}");
        assert!(root.join(".changeset/typo.md").exists());
    }
}
