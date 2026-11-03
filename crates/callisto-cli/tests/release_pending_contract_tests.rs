//! The release workflow reads `callisto --format json release pending` with jq (`.pending[0].source`).
//! A renamed key would read as "nothing pending" and skip a stuck release, so the real output is pinned here.
#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;

use release_harness::*;

/// A workspace whose HEAD is a release commit (a committed decision, its changesets consumed) with no release tag.
fn unfinished_release() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/core\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/core/src")).unwrap();
    fs::write(
        root.join("crates/core/Cargo.toml"),
        "[package]\nname = \"core-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("crates/core/src/lib.rs"), "").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial workspace"]);
    git(root, &["tag", "-a", "-m", "release", "core-crate@0.1.0"]);

    let init = callisto(root, &["init", "--yes", "--versioning", "independent"]);
    assert!(init.status.success(), "init: {}", String::from_utf8_lossy(&init.stderr));
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "configure callisto"]);
    let add = callisto(
        root,
        &["add", "--package", "cargo/core-crate:minor", "--summary", "Ship"],
    );
    assert!(add.status.success(), "add: {}", String::from_utf8_lossy(&add.stderr));
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "add changeset"]);
    let version = callisto(root, &["version", "--emit-decision", DECISION_PATH]);
    assert!(
        version.status.success(),
        "version: {}",
        String::from_utf8_lossy(&version.stderr)
    );
    for entry in fs::read_dir(root.join(".changeset")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|n| n != "README.md") && path.extension().is_some_and(|e| e == "md") {
            fs::remove_file(path).unwrap();
        }
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "release"]);
    let release_commit = git(root, &["rev-parse", "HEAD"]);
    (dir, release_commit)
}

#[test]
fn release_pending_json_carries_the_keys_the_workflow_reads() {
    let (dir, release_commit) = unfinished_release();
    let out = callisto(dir.path(), &["release", "pending"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();

    // `.pending[0].source` is what callisto-release.yml selects as the release to finish.
    assert_eq!(report["pending"][0]["source"], release_commit, "{report}");
    assert_eq!(
        report["pending"][0]["packages"][0]["package"], "cargo/core-crate",
        "{report}"
    );
    assert_eq!(report["pending"][0]["packages"][0]["version"], "0.2.0", "{report}");
    let missing = report["pending"][0]["missing"].as_array().expect("missing is an array");
    assert!(
        missing
            .iter()
            .any(|gap| gap.as_str().is_some_and(|gap| gap.contains("core-crate@0.2.0"))),
        "{report}"
    );
}

#[test]
fn release_pending_json_is_an_empty_list_when_every_release_finished() {
    let (dir, _) = unfinished_release();
    git(dir.path(), &["tag", "-a", "-m", "release", "core-crate@0.2.0"]);
    let out = callisto(dir.path(), &["release", "pending"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["pending"], serde_json::json!([]), "{report}");
}
