//! `status --since <ref>`: a package changed on a branch must be named by a changeset before the branch merges.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use callisto_fixtures::git::{init_repo, run_git};

fn status_since(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args([
            "--cwd",
            root.to_str().unwrap(),
            "--format",
            "json",
            "status",
            "--check",
            "--since",
            "main",
        ])
        .output()
        .unwrap()
}

fn flagged(output: &Output) -> Vec<String> {
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    // An empty list is left out of the report.
    report["diagnostics"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|d| d["code"] == "changed-without-changeset")
        .map(|d| d["package"].as_str().unwrap().to_string())
        .collect()
}

fn commit(root: &Path, message: &str) {
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", message]);
}

/// Two Cargo packages, `a` and `b`, in a fresh repository on `main`.
fn two_packages(root: &Path) {
    init_repo(root);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"a\", \"b\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    for name in ["a", "b"] {
        fs::create_dir_all(root.join(name).join("src")).unwrap();
        fs::write(
            root.join(name).join("Cargo.toml"),
            format!("[package]\nname = \"pkg-{name}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        fs::write(root.join(name).join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    }
}

/// Only a changeset the loader reads counts: a nested file of the same name does not cover a pending one.
#[test]
fn a_nested_file_named_like_a_pending_changeset_is_not_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    two_packages(root);
    fs::create_dir_all(root.join(".changeset/archive")).unwrap();
    fs::write(root.join(".changeset/old.md"), "---\ncargo/pkg-a: patch\n---\n\nOld.\n").unwrap();
    commit(root, "initial");
    run_git(root, &["checkout", "-q", "-b", "feature"]);
    fs::write(root.join("a/src/lib.rs"), "pub fn f() { let _ = 1; }\n").unwrap();
    fs::write(root.join(".changeset/archive/old.md"), "notes\n").unwrap();
    commit(root, "change a");
    assert_eq!(flagged(&status_since(root)), ["cargo/pkg-a"]);
}

/// A bump inferred from the branch's commits covers the change though an older changeset decides the severity.
#[test]
fn an_inferred_bump_covers_a_change_when_an_older_changeset_wins_the_severity() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    two_packages(root);
    fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"cargo/pkg-a\"\nrelease-trigger = \"auto\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(root.join(".changeset/big.md"), "---\ncargo/pkg-a: major\n---\n\nBig.\n").unwrap();
    commit(root, "chore: initial");
    run_git(root, &["checkout", "-q", "-b", "feature"]);
    fs::write(root.join("a/src/lib.rs"), "pub fn f() { let _ = 1; }\n").unwrap();
    commit(root, "fix: tweak a");
    let output = status_since(root);
    assert!(
        flagged(&output).is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn a_branch_change_without_a_changeset_fails_status_since() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    two_packages(root);
    // A changeset already on main belongs to an earlier change; it does not cover this branch's.
    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(
        root.join(".changeset/earlier.md"),
        "---\ncargo/pkg-a: patch\n---\n\nAn earlier fix.\n",
    )
    .unwrap();
    commit(root, "initial");
    run_git(root, &["checkout", "-q", "-b", "feature"]);

    let clean = status_since(root);
    assert!(clean.status.success(), "{}", String::from_utf8_lossy(&clean.stderr));
    assert!(flagged(&clean).is_empty());

    fs::write(root.join("a/src/lib.rs"), "pub fn f() { let _ = 1; }\n").unwrap();
    commit(root, "change a");
    let missing = status_since(root);
    assert_eq!(
        missing.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&missing.stderr)
    );
    assert_eq!(flagged(&missing), ["cargo/pkg-a"]);

    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(
        root.join(".changeset/quiet-change.md"),
        "---\ncargo/pkg-a: none\n---\n\nInternal refactor.\n",
    )
    .unwrap();
    let named = status_since(root);
    assert!(named.status.success(), "{}", String::from_utf8_lossy(&named.stderr));
    assert!(flagged(&named).is_empty());

    let unknown = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--cwd", root.to_str().unwrap(), "status", "--since", "no-such-branch"])
        .output()
        .unwrap();
    assert!(!unknown.status.success());
}
