//! A Cargo workspace below the repository root is released like any other: members inherit from it, its lockfile is
//! refreshed and staged beside it, and a release commit verifies against it.
#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;
use std::path::Path;
use std::process::Command;

use release_harness::{callisto, git, DECISION_PATH};

/// An npm workspace at the root and a Rust workspace in `native/`: `a` inherits its version, `b` depends on `a` through
/// `[workspace.dependencies]`. There is no `Cargo.toml` at the repository root.
fn nested_workspace_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    git(
        root,
        &["remote", "add", "origin", "https://github.com/example/nested.git"],
    );
    fs::write(
        root.join("package.json"),
        r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("packages/web")).unwrap();
    fs::write(
        root.join("packages/web/package.json"),
        r#"{"name":"web","version":"2.0.0"}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("native/crates/a/src")).unwrap();
    fs::create_dir_all(root.join("native/crates/b/src")).unwrap();
    fs::write(
        root.join("native/Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n\n[workspace.package]\nversion = \"0.3.0\"\nedition = \"2021\"\n\n\
         [workspace.dependencies]\na = { path = \"crates/a\", version = \"0.3.0\" }\n",
    )
    .unwrap();
    fs::write(
        root.join("native/crates/a/Cargo.toml"),
        "[package]\nname = \"a\"\nversion.workspace = true\nedition.workspace = true\n",
    )
    .unwrap();
    fs::write(
        root.join("native/crates/b/Cargo.toml"),
        "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition.workspace = true\n\n[dependencies]\na = { workspace = true }\n",
    )
    .unwrap();
    fs::write(root.join("native/crates/a/src/lib.rs"), "\n").unwrap();
    fs::write(root.join("native/crates/b/src/lib.rs"), "\n").unwrap();
    assert!(Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .arg("generate-lockfile")
        .current_dir(root.join("native"))
        .status()
        .unwrap()
        .success());
    fs::write(root.join(".gitignore"), "target\n").unwrap();
    commit(root, "initial workspace");
    dir
}

fn commit(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", message]);
}

fn ok(root: &Path, args: &[&str]) -> serde_json::Value {
    let output = callisto(root, args);
    assert!(
        output.status.success(),
        "callisto {args:?} failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null)
}

#[test]
fn status_add_and_version_work_without_a_cargo_manifest_at_the_root() {
    let dir = nested_workspace_repo();
    let root = dir.path();

    let status = ok(root, &["status"]).to_string();
    assert!(status.contains("0.3.0") && status.contains("0.1.0"), "{status}");

    ok(
        root,
        &["add", "--package", "cargo/a:minor", "--summary", "A new feature"],
    );
    commit(root, "changeset");
    ok(root, &["version"]);

    let workspace = fs::read_to_string(root.join("native/Cargo.toml")).unwrap();
    assert!(
        workspace.contains("a = { path = \"crates/a\", version = \"0.4.0\" }"),
        "{workspace}"
    );
    assert!(fs::read_to_string(root.join("native/crates/a/Cargo.toml"))
        .unwrap()
        .contains("version = \"0.4.0\""));
    assert!(fs::read_to_string(root.join("native/crates/b/Cargo.toml"))
        .unwrap()
        .contains("version = \"0.1.1\""));

    assert!(
        !root.join("Cargo.lock").exists(),
        "no lockfile belongs at the repository root"
    );
    let staged = git(root, &["diff", "--cached", "--name-only"]);
    assert!(staged.lines().any(|path| path == "native/Cargo.lock"), "{staged}");
    let lock = fs::read_to_string(root.join("native/Cargo.lock")).unwrap();
    assert!(lock.contains("name = \"a\"\nversion = \"0.4.0\""), "{lock}");

    // The workspace resolves with its locked install, as after any `version`.
    let metadata = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(root.join("native/Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        metadata.status.success(),
        "{}",
        String::from_utf8_lossy(&metadata.stderr)
    );
}

/// The release commit is verified against the inherited version at both of its sides.
#[test]
fn a_release_commit_in_a_nested_workspace_verifies_the_version_it_inherited() {
    let dir = nested_workspace_repo();
    let root = dir.path();
    let init = callisto(root, &["init", "--yes", "--versioning", "independent"]);
    assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));
    commit(root, "configure callisto");

    ok(
        root,
        &["add", "--package", "cargo/a:minor", "--summary", "A new feature"],
    );
    commit(root, "changeset");
    ok(root, &["version", "--emit-decision", DECISION_PATH]);
    for entry in fs::read_dir(root.join(".changeset")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|name| name != "README.md") && path.extension().is_some_and(|ext| ext == "md") {
            fs::remove_file(path).unwrap();
        }
    }
    commit(root, "release a 0.4.0");
    let release_commit = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "--detach", &release_commit]);

    let external = tempfile::tempdir().unwrap();
    let intent = external.path().join("release-intent.json");
    let plan = callisto(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            &release_commit,
            "--decision",
            DECISION_PATH,
            "--out",
            intent.to_str().unwrap(),
        ],
    );
    assert!(plan.status.success(), "{}", String::from_utf8_lossy(&plan.stderr));
    let intent: serde_json::Value = serde_json::from_slice(&fs::read(&intent).unwrap()).unwrap();
    let entries = intent["decision"]["entries"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry["package"] == "cargo/a" && entry["targetVersion"] == "0.4.0"),
        "{entries:?}"
    );
}
