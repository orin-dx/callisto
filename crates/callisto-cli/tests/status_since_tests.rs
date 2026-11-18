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

#[test]
fn a_branch_change_without_a_changeset_fails_status_since() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
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
