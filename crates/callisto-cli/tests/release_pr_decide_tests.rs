//! `release-pr decide` against real workspaces: what opens a release PR and what fails before one.

use std::path::Path;
use std::process::{Command, Output};

use callisto_fixtures::git::{init_repo, run_git};

/// One auto-released Cargo package tagged at 1.0.0.
fn tagged_auto_package(root: &Path) {
    init_repo(root);
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"pkg-alpha\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"cargo/pkg-alpha\"\nrelease-trigger = \"auto\"\n",
    )
    .unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "chore: add package"]);
    run_git(root, &["tag", "-m", "release", "pkg-alpha@1.0.0"]);
}

fn decide(root: &Path) -> Output {
    let repository = callisto_model::GitHubRepository::parse("example/widget").unwrap();
    let snapshot = callisto_model::ReleasePrSnapshotV2::new(
        repository,
        "main".into(),
        callisto_model::CommitSha::parse(&"a".repeat(40)).unwrap(),
        vec![],
    )
    .unwrap();
    Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args([
            "--cwd",
            root.to_str().unwrap(),
            "--format",
            "json",
            "release-pr",
            "decide",
        ])
        .args(["--snapshot", &serde_json::to_string(&snapshot).unwrap()])
        .args(["--repository", "example/widget", "--base-branch", "main"])
        .args(["--release-branch", "callisto/version-packages"])
        .output()
        .unwrap()
}

fn action_kind(output: &Output) -> String {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    report["action"]["kind"].as_str().unwrap().to_string()
}

#[test]
fn a_release_inferred_from_commits_alone_opens_a_release_pr() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    tagged_auto_package(root);
    assert_eq!(action_kind(&decide(root)), "noop");

    std::fs::write(root.join("lib.rs"), "pub fn feature() {}\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "feat: add a feature"]);
    assert_eq!(action_kind(&decide(root)), "create");
}

/// Inference starts at the last tag, so the generated workflow runs decide only after execute tags a release merge:
/// before the tag it would propose the released commits again.
#[test]
fn a_merged_inferred_release_is_settled_by_its_tag() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    tagged_auto_package(root);
    std::fs::write(root.join("lib.rs"), "pub fn feature() {}\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "feat: add a feature"]);
    let version = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--cwd", root.to_str().unwrap(), "version", "--no-refresh-lockfiles"])
        .output()
        .unwrap();
    assert!(version.status.success(), "{}", String::from_utf8_lossy(&version.stderr));
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "chore(release): version packages"]);
    assert_eq!(
        action_kind(&decide(root)),
        "create",
        "untagged, the released feat commit is inferred again"
    );

    run_git(root, &["tag", "-m", "release", "pkg-alpha@1.1.0"]);
    assert_eq!(action_kind(&decide(root)), "noop");
}

#[test]
fn an_unparseable_changeset_fails_decide_instead_of_a_noop() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    tagged_auto_package(root);
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(
        root.join(".changeset/broken.md"),
        "---\npkg-alpha: sideways\n---\n\nBroken.\n",
    )
    .unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "chore: add a broken changeset"]);

    let output = decide(root);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("broken.md"), "{stderr}");
}
