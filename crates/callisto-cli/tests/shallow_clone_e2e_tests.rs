//! Commit-based inference reads history; in a shallow clone it would read a shorter one and plan a smaller bump.
#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;
use std::path::Path;

use release_harness::{callisto, git};

/// A crate with three commits; `auto` makes it infer its bump from commits.
fn history(auto: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "\n").unwrap();
    if auto {
        fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"lib\"\nrelease-trigger = \"auto\"\n",
        )
        .unwrap();
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "chore: start"]);
    for message in ["feat: one", "feat: two"] {
        fs::write(root.join("src/lib.rs"), format!("// {message}\n")).unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-m", message]);
    }
    dir
}

fn shallow_clone(source: &Path) -> tempfile::TempDir {
    let clone = tempfile::tempdir().unwrap();
    git(
        clone.path(),
        &["clone", "--depth", "1", &format!("file://{}", source.display()), "."],
    );
    assert_eq!(git(clone.path(), &["rev-parse", "--is-shallow-repository"]), "true");
    clone
}

fn stdout_and_stderr(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn inference_in_a_shallow_clone_is_refused_not_planned_from_partial_history() {
    let source = history(true);
    let clone = shallow_clone(source.path());

    let status = callisto(clone.path(), &["status"]);
    assert!(!status.status.success());
    assert!(
        stdout_and_stderr(&status).contains("E315"),
        "{}",
        stdout_and_stderr(&status)
    );

    let full = callisto(source.path(), &["status"]);
    assert!(full.status.success(), "{}", stdout_and_stderr(&full));
}

#[test]
fn a_shallow_clone_is_fine_when_no_package_infers_from_commits() {
    let source = history(false);
    let clone = shallow_clone(source.path());

    let status = callisto(clone.path(), &["status"]);
    assert!(status.status.success(), "{}", stdout_and_stderr(&status));
}
