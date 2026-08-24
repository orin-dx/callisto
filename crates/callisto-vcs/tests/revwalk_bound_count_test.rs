//! Regression test for `commits_since_with_pathspec`'s revwalk bounding.
//! Isolated in its own integration-test binary because
//! `callisto_vcs::revwalk_visit_count` is a process-global counter that
//! other, non-`#[serial]` tests in a shared binary would pollute (see
//! `crates/callisto-graph/tests/apply_persist_open_count_test.rs` for the
//! precedent this file follows).

use std::path::Path;

use callisto_model::CommitSha;
use callisto_vcs::GitRepository;
use serial_test::serial;

fn run_git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git must be installed to run this test");
    assert!(status.success(), "git {args:?} failed in {dir:?}");
}

fn init_repo(root: &Path) {
    run_git(root, &["init", "-q"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Test"]);
}

fn commit(root: &Path, n: usize) {
    std::fs::write(root.join("f.txt"), format!("commit {n}\n")).unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", &format!("commit {n}")]);
}

/// Spec: `commits_since_with_pathspec` must bound its walk to `since..HEAD`
/// -- not walk the whole repository history and filter after the fact. 30
/// linear commits precede `since`; 3 more follow it. Under the old
/// two-full-walk-then-exclude implementation, the primary revwalk visits
/// every one of the 33 commits reachable from HEAD regardless of `since`.
/// The bounded implementation must visit only the small number of commits
/// between `since` and HEAD.
#[test]
#[serial]
fn commits_since_with_pathspec_bounds_walk_to_since_head_range() {
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();
    init_repo(root);

    for n in 1..=30 {
        commit(root, n);
    }

    let since_output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(since_output.status.success());
    let since_sha = CommitSha::parse(String::from_utf8_lossy(&since_output.stdout).trim()).unwrap();

    for n in 31..=33 {
        commit(root, n);
    }

    let repo = GitRepository::discover(root).unwrap();

    callisto_vcs::reset_revwalk_visit_count();
    let commits = repo.commits_since_with_pathspec(Some(&since_sha), &[]).unwrap();

    assert_eq!(commits.len(), 3, "must return exactly the 3 commits after `since`");

    assert_eq!(
        callisto_vcs::revwalk_visit_count(),
        3,
        "revwalk must visit exactly the 3 commits between `since` and HEAD, not the full 33-commit history"
    );
}
