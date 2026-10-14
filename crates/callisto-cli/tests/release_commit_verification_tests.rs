#![cfg(unix)]

//! Black-box tests that `release plan --from-release-commit` refuses a release commit
//! that does not match the decision it claims to satisfy. The committed decision is the
//! release authority, so every way a commit can disagree with it must fail closed with
//! E165 before any intent exists.

#[path = "common/release_harness.rs"]
mod release_harness;

use std::{fs, path::Path, process::Output};

use release_harness::*;

const MANIFEST: &str = "crates/core/Cargo.toml";

/// Builds a commit on the release commit's parent that applies the release commit's
/// changes except for `skip` (restored to the parent's content) and `overrides`
/// (written with different content), then returns its SHA.
fn tampered_release_commit(root: &Path, release_commit: &str, skip: &[&str], overrides: &[(&str, &str)]) -> String {
    let parent = git(root, &["rev-parse", &format!("{release_commit}^")]);
    let delta = git(
        root,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-status",
            "-r",
            &parent,
            release_commit,
        ],
    );
    git(root, &["checkout", "--detach", &parent]);
    for line in delta.lines() {
        let (status, path) = line.split_once('\t').expect("name-status line");
        if status == "D" {
            git(root, &["rm", "-q", "--", path]);
        } else {
            git(root, &["checkout", release_commit, "--", path]);
        }
    }
    for path in skip {
        if git(root, &["ls-tree", &parent, "--", path]).is_empty() {
            git(root, &["rm", "-q", "-f", "--", path]);
        } else {
            git(root, &["checkout", &parent, "--", path]);
        }
    }
    for (path, content) in overrides {
        fs::write(root.join(path), content).unwrap();
    }
    git(root, &["add", "-A"]);
    git(
        root,
        &["commit", "-q", "--allow-empty", "-m", "tampered release commit"],
    );
    let sha = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "--detach", &sha]);
    sha
}

/// The path the release commit adds or modifies that ends with `suffix`.
fn changed_path(root: &Path, release_commit: &str, suffix: &str) -> String {
    git(
        root,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            &format!("{release_commit}^"),
            release_commit,
        ],
    )
    .lines()
    .find(|path| path.ends_with(suffix))
    .unwrap_or_else(|| panic!("release commit changes no `{suffix}`"))
    .to_owned()
}

fn plan(root: &Path, external: &Path, commit: &str) -> Output {
    callisto(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            commit,
            "--decision",
            DECISION_PATH,
            "--out",
            external.join("intent.json").to_str().unwrap(),
        ],
    )
}

fn assert_rejected(root: &Path, external: &Path, commit: &str, what: &str, reason: &str) {
    let out = plan(root, external, commit);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{what} must be rejected: {stderr}");
    assert!(stderr.contains("E165"), "{what} must fail with E165: {stderr}");
    assert!(stderr.contains(reason), "{what} must fail because `{reason}`: {stderr}");
    assert!(
        !external.join("intent.json").exists(),
        "{what} must not leave an intent behind"
    );
}

#[test]
fn an_untampered_release_commit_still_plans() {
    let (dir, release_commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let out = plan(dir.path(), external.path(), &release_commit);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn a_commit_that_does_not_write_the_decision_is_rejected() {
    let (dir, release_commit) = release_commit_fixture();
    let root = dir.path();
    fs::write(root.join("crates/core/src/lib.rs"), "pub fn core() { /* later */ }\n").unwrap();
    git(root, &["commit", "-q", "-am", "unrelated follow-up"]);
    let follow_up = git(root, &["rev-parse", "HEAD"]);
    assert_ne!(follow_up, release_commit);
    let external = tempfile::tempdir().unwrap();
    assert_rejected(
        root,
        external.path(),
        &follow_up,
        "a commit that carries a stale decision",
        "release-decision file was not written",
    );
}

#[test]
fn a_commit_that_consumes_no_changeset_is_rejected() {
    let (dir, release_commit) = release_commit_fixture();
    let root = dir.path();
    let changesets: Vec<String> = git(
        root,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "--diff-filter=D",
            "-r",
            &format!("{release_commit}^"),
            &release_commit,
        ],
    )
    .lines()
    .map(str::to_owned)
    .collect();
    assert!(!changesets.is_empty(), "fixture must consume a changeset");
    let skip: Vec<&str> = changesets.iter().map(String::as_str).collect();
    let commit = tampered_release_commit(root, &release_commit, &skip, &[]);
    let external = tempfile::tempdir().unwrap();
    assert_rejected(
        root,
        external.path(),
        &commit,
        "a release commit that leaves the changeset in place",
        "no changeset was consumed",
    );
}

#[test]
fn a_commit_that_does_not_touch_the_changelog_is_rejected() {
    let (dir, release_commit) = release_commit_fixture();
    let root = dir.path();
    let changelog = changed_path(root, &release_commit, "CHANGELOG.md");
    let commit = tampered_release_commit(root, &release_commit, &[&changelog], &[]);
    let external = tempfile::tempdir().unwrap();
    assert_rejected(
        root,
        external.path(),
        &commit,
        "a release commit without its changelog entry",
        "changelog was not touched",
    );
}

#[test]
fn a_commit_that_does_not_change_the_released_manifest_is_rejected() {
    let (dir, release_commit) = release_commit_fixture();
    let root = dir.path();
    let commit = tampered_release_commit(root, &release_commit, &[MANIFEST], &[]);
    let external = tempfile::tempdir().unwrap();
    assert_rejected(
        root,
        external.path(),
        &commit,
        "a release commit that leaves the manifest version alone",
        "manifest is not part of this commit",
    );
}

#[test]
fn a_commit_whose_manifest_version_differs_from_the_decision_is_rejected() {
    let (dir, release_commit) = release_commit_fixture();
    let root = dir.path();
    let wrong = "[package]\nname = \"core-crate\"\nversion = \"0.9.0\"\nedition = \"2021\"\n";
    let commit = tampered_release_commit(root, &release_commit, &[], &[(MANIFEST, wrong)]);
    let external = tempfile::tempdir().unwrap();
    assert_rejected(
        root,
        external.path(),
        &commit,
        "a release commit that sets a different version than the decision",
        "manifest version does not match",
    );
}
