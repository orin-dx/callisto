//! A private npm workspace root is a script container, not a release unit, unless `[[package]]` names it.
#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;
use std::path::Path;

use release_harness::*;

const NOTE: &str = "ignored private workspace root `npm/monorepo` (name it in [[package]] to version it)";

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// A git repo with an npm root `package.json` of `root_manifest`, one member `app`, and an optional `callisto.toml`.
fn repo(root_manifest: &str, callisto_toml: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    write(root, "package.json", root_manifest);
    write(
        root,
        "packages/app/package.json",
        r#"{"name":"app","version":"1.0.0","private":true}"#,
    );
    if let Some(toml) = callisto_toml {
        write(root, "callisto.toml", toml);
    }
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "init"]);
    dir
}

/// The package ids `status` lists and what it printed to stderr.
fn status(dir: &tempfile::TempDir) -> (String, String) {
    let out = callisto(dir.path(), &["status"]);
    assert!(out.status.success(), "stderr: {}", stderr_of(&out));
    let stderr = stderr_of(&out);
    (String::from_utf8(out.stdout).unwrap(), stderr)
}

const PRIVATE_WORKSPACE: &str = r#"{"name":"monorepo","private":true,"version":"0.0.0","workspaces":["packages/*"]}"#;

#[test]
fn a_private_versioned_workspace_root_is_not_a_package_and_status_says_so() {
    let (stdout, stderr) = status(&repo(PRIVATE_WORKSPACE, None));
    assert!(!stdout.contains("monorepo"), "stdout: {stdout}");
    assert!(stdout.contains("app"), "stdout: {stdout}");
    assert_eq!(stderr.matches(NOTE).count(), 1, "stderr: {stderr}");
}

#[test]
fn a_private_workspace_root_named_in_a_package_block_is_a_package() {
    let dir = repo(
        PRIVATE_WORKSPACE,
        Some("[[package]]\nmatch = \"npm/monorepo\"\nrelease-trigger = \"changeset\"\n"),
    );
    let (stdout, stderr) = status(&dir);
    assert!(stdout.contains("monorepo"), "stdout: {stdout}");
    assert!(!stderr.contains(NOTE), "stderr: {stderr}");
}

#[test]
fn a_public_workspace_root_is_still_a_package() {
    let (stdout, stderr) = status(&repo(
        r#"{"name":"monorepo","version":"0.0.0","workspaces":["packages/*"]}"#,
        None,
    ));
    assert!(stdout.contains("monorepo"), "stdout: {stdout}");
    assert!(!stderr.contains(NOTE), "stderr: {stderr}");
}

#[test]
fn a_private_root_without_workspaces_is_still_a_package() {
    let (stdout, stderr) = status(&repo(r#"{"name":"monorepo","private":true,"version":"0.0.0"}"#, None));
    assert!(stdout.contains("monorepo"), "stdout: {stdout}");
    assert!(!stderr.contains(NOTE), "stderr: {stderr}");
}

#[test]
fn a_private_workspace_member_is_still_a_package() {
    let (stdout, stderr) = status(&repo(PRIVATE_WORKSPACE, None));
    assert!(stdout.contains("app"), "stdout: {stdout}");
    assert!(!stderr.contains("`npm/app`"), "stderr: {stderr}");
}
