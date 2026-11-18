#![cfg(unix)]

//! A whole prerelease cycle through the release pull request: each `release-pr decide` opens a PR only when the
//! cycle has something new, and each merged version commit verifies as release authority.

#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;
use std::path::Path;

use release_harness::*;

fn decide(root: &Path) -> String {
    let snapshot = callisto_model::ReleasePrSnapshotV2::new(
        callisto_model::GitHubRepository::parse("example/core-crate").unwrap(),
        "main".into(),
        callisto_model::CommitSha::parse(&"a".repeat(40)).unwrap(),
        vec![],
    )
    .unwrap();
    let out = callisto(
        root,
        &[
            "release-pr",
            "decide",
            "--snapshot",
            &serde_json::to_string(&snapshot).unwrap(),
            "--repository",
            "example/core-crate",
            "--base-branch",
            "main",
            "--release-branch",
            "callisto/version-packages",
        ],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    report["action"]["kind"].as_str().unwrap().to_string()
}

/// Opens, merges and verifies one release PR; returns the version it released.
fn release(root: &Path) -> String {
    assert_eq!(decide(root), "create");
    let version = callisto(
        root,
        &["version", "--no-refresh-lockfiles", "--emit-decision", DECISION_PATH],
    );
    assert!(version.status.success(), "{}", String::from_utf8_lossy(&version.stderr));
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "chore(release): version packages"]);
    let commit = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "-q", "--detach", &commit]);
    let external = tempfile::tempdir().unwrap();
    let plan = callisto(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            &commit,
            "--decision",
            DECISION_PATH,
            "--out",
            external.path().join("intent.json").to_str().unwrap(),
        ],
    );
    assert!(plan.status.success(), "{}", String::from_utf8_lossy(&plan.stderr));
    git(root, &["checkout", "-q", "main"]);
    git(root, &["merge", "-q", "--ff-only", &commit]);
    let decision: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join(DECISION_PATH)).unwrap()).unwrap();
    decision["entries"][0]["targetVersion"].as_str().unwrap().to_string()
}

fn add(root: &Path, bump: &str, summary: &str) {
    let out = callisto(
        root,
        &[
            "add",
            "--package",
            &format!("cargo/core-crate:{bump}"),
            "--summary",
            summary,
        ],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", summary]);
}

#[test]
fn a_prerelease_cycle_runs_end_to_end_through_release_pull_requests() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    git(
        root,
        &["remote", "add", "origin", "https://github.com/example/core-crate.git"],
    );
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"core-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\ndescription = \"t\"\nlicense = \"MIT\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn core() {}\n").unwrap();
    fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"cargo/core-crate\"\npublish-to = [\"crates-io\"]\n",
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "initial"]);

    let enter = callisto(root, &["pre", "enter", "beta"]);
    assert!(enter.status.success(), "{}", String::from_utf8_lossy(&enter.stderr));
    git(root, &["commit", "-q", "-am", "enter beta"]);
    assert_eq!(decide(root), "noop", "entering a cycle releases nothing on its own");

    add(root, "minor", "Add a feature");
    assert_eq!(release(root), "1.1.0-beta.0");

    add(root, "patch", "Fix the feature");
    assert_eq!(release(root), "1.1.0-beta.1");

    assert_eq!(decide(root), "noop", "a cycle with nothing new opens no pull request");

    let exit = callisto(root, &["pre", "exit"]);
    assert!(exit.status.success(), "{}", String::from_utf8_lossy(&exit.stderr));
    git(root, &["commit", "-q", "-am", "exit beta"]);
    assert_eq!(release(root), "1.1.0");
    assert!(!root.join(".changeset/pre.json").exists());

    assert_eq!(decide(root), "noop");
}
