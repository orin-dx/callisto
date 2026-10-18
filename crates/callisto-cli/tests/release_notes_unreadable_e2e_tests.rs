//! A changelog that exists but cannot be read stops release planning; an intact one plans as before.
#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;
use std::path::Path;

use release_harness::{callisto, git, plan_intent, release_commit_fixture, DECISION_PATH};

/// Re-commits the release commit with the changelog replaced by `bytes`, and returns its SHA.
fn amend_changelog(root: &Path, bytes: &[u8]) -> String {
    git(root, &["checkout", "-q", "--detach", "HEAD"]);
    fs::write(root.join("crates/core/CHANGELOG.md"), bytes).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "--amend", "--no-edit", "-q"]);
    git(root, &["rev-parse", "HEAD"])
}

#[test]
fn an_intact_changelog_plans_and_one_that_cannot_be_read_does_not() {
    let (dir, release_commit) = release_commit_fixture();
    let root = dir.path();
    let external = tempfile::tempdir().unwrap();
    plan_intent(root, external.path(), &release_commit);

    let broken = amend_changelog(root, b"## 0.2.0\n\n- caf\xe9\n");
    let intent = external.path().join("broken-intent.json");
    let planned = callisto(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            &broken,
            "--decision",
            DECISION_PATH,
            "--out",
            intent.to_str().unwrap(),
        ],
    );
    assert!(!planned.status.success());
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&planned.stdout),
        String::from_utf8_lossy(&planned.stderr)
    );
    assert!(output.contains("E125") && output.contains("CHANGELOG.md"), "{output}");
    assert!(!intent.exists(), "no intent is written");
}
