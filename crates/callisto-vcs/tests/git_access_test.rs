//! `GitAccess` against real repositories through the system `git` binary.

use std::path::{Path, PathBuf};

use callisto_model::{ApplyPermit, CommandError, CommandOutput, CommandRunner, CommitSha};
use callisto_vcs::{GitAccess, TagSignPolicy, VcsError};

/// Runs real `git`, isolated from the developer's global and system config.
struct SystemGit;

impl CommandRunner for SystemGit {
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
        let output = hermetic(program)
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|e| CommandError::Io {
                program: program.to_string(),
                message: e.to_string(),
            })?;
        Ok(CommandOutput {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn hermetic(program: &str) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_AUTHOR_NAME")
        .env_remove("GIT_AUTHOR_EMAIL")
        .env_remove("GIT_COMMITTER_NAME")
        .env_remove("GIT_COMMITTER_EMAIL")
        .env_remove("EMAIL");
    command
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = hermetic("git").args(args).current_dir(dir).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn init_repo(root: &Path) {
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "user.name", "Test"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["config", "tag.gpgsign", "false"]);
}

fn commit_file(root: &Path, path: &str, contents: &[u8], message: &str) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, contents).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", message]);
}

fn head(root: &Path) -> CommitSha {
    CommitSha::parse(&git(root, &["rev-parse", "HEAD"])).unwrap()
}

fn summaries(root: &Path, since: Option<&str>, paths: &[&str]) -> Vec<String> {
    let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    GitAccess::new(root, &SystemGit)
        .commits_since(since, &paths)
        .unwrap()
        .into_iter()
        .map(|c| c.summary)
        .collect()
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[test]
fn commits_since_filters_by_changed_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    commit_file(root, "crates/pkg-a/file.txt", b"a\n", "feat: add pkg-a");
    commit_file(root, "crates/pkg-b/file.txt", b"b\n", "feat: add pkg-b");
    commit_file(root, "crates/pkg-a/file.txt", b"a2\n", "fix: tweak pkg-a");

    assert_eq!(
        summaries(root, None, &["crates/pkg-a"]),
        ["fix: tweak pkg-a", "feat: add pkg-a"]
    );
    assert!(summaries(root, None, &["crates/pkg-c"]).is_empty());
    assert_eq!(summaries(root, None, &[]).len(), 3);
}

/// `"."` is the pathspec a root-level manifest gets; it must match root files.
#[test]
fn commits_since_root_pathspec_matches_root_level_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    commit_file(root, "Cargo.toml", b"[package]\n", "chore: add package");
    commit_file(
        root,
        "Cargo.toml",
        b"[package]\nname = \"p\"\n",
        "feat: add a new feature",
    );

    assert_eq!(
        summaries(root, None, &["."]),
        ["feat: add a new feature", "chore: add package"]
    );
}

#[test]
fn commits_since_bound_is_exclusive() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    commit_file(root, "a.txt", b"1\n", "feat: c1");
    let since = head(root);
    commit_file(root, "a.txt", b"2\n", "feat: c2");
    commit_file(root, "a.txt", b"3\n", "feat: c3");

    assert_eq!(
        summaries(root, Some(since.as_str()), &["a.txt"]),
        ["feat: c3", "feat: c2"]
    );
}

#[test]
fn commits_since_unresolvable_bound_is_ref_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    commit_file(root, "a.txt", b"1\n", "feat: c1");

    let result = GitAccess::new(root, &SystemGit).commits_since(Some("refs/tags/missing"), &[]);

    assert!(
        matches!(result, Err(VcsError::RefNotFound { ref ref_name }) if ref_name == "refs/tags/missing"),
        "got {result:?}"
    );
}

/// Merge commits are excluded; commits on both sides of the merge are kept.
#[test]
fn commits_since_excludes_merge_commits() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    commit_file(root, "a.txt", b"a\n", "feat: c1 on main");
    git(root, &["checkout", "-q", "-b", "feature"]);
    commit_file(root, "b.txt", b"b\n", "feat: c2 on feature");
    git(root, &["checkout", "-q", "main"]);
    commit_file(root, "c.txt", b"c\n", "feat: c3 on main");
    git(root, &["merge", "--no-ff", "-q", "-m", "merge: feature", "feature"]);

    assert_eq!(
        sorted(summaries(root, None, &[])),
        ["feat: c1 on main", "feat: c2 on feature", "feat: c3 on main"]
    );
}

/// A branch forked before the `since` tag and merged after it contributes
/// its commits: `since..HEAD` excludes only what the tag can reach.
///
/// ```text
/// A - B - S(v1) - C - M
///      \             /
///       D --------- E
/// ```
#[test]
fn commits_since_includes_branch_forked_before_the_bound() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    commit_file(root, "base.txt", b"a\n", "feat: A");
    commit_file(root, "b.txt", b"b\n", "feat: B");
    let b = head(root);
    commit_file(root, "s.txt", b"s\n", "chore: S release");
    git(root, &["tag", "-a", "-m", "v1", "v1"]);
    git(root, &["checkout", "-q", "-b", "feat", b.as_str()]);
    commit_file(root, "d.txt", b"d\n", "feat: D");
    commit_file(root, "e.txt", b"e\n", "feat: E");
    git(root, &["checkout", "-q", "main"]);
    commit_file(root, "c.txt", b"c\n", "feat: C");
    git(root, &["merge", "--no-ff", "-q", "-m", "merge: feat", "feat"]);

    assert_eq!(
        sorted(summaries(root, Some("refs/tags/v1"), &[])),
        ["feat: C", "feat: D", "feat: E"]
    );
}

/// `--full-history`: a side-branch commit touching the path counts even when
/// the merge discarded its change. Default `git log` history simplification
/// would drop it, since the merge is TREESAME to its first parent.
#[test]
fn commits_since_keeps_commits_whose_change_a_merge_discarded() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    commit_file(root, "pkg-a/f.txt", b"a\n", "feat: a1");
    git(root, &["tag", "-a", "-m", "v1", "v1"]);
    git(root, &["checkout", "-q", "-b", "side"]);
    commit_file(root, "pkg-a/side.txt", b"s\n", "feat: side touches a");
    git(root, &["checkout", "-q", "main"]);
    commit_file(root, "pkg-b/g.txt", b"b\n", "fix: b");
    git(root, &["merge", "-q", "-s", "ours", "-m", "merge: ours", "side"]);

    assert_eq!(
        summaries(root, Some("refs/tags/v1"), &["pkg-a"]),
        ["feat: side touches a"]
    );
}

/// No rename detection: moving a file out of a path touches that path.
#[test]
fn commits_since_counts_a_rename_out_of_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    commit_file(root, "crates/pkg-a/file.txt", b"a\n", "feat: add pkg-a");
    std::fs::create_dir_all(root.join("crates/pkg-b")).unwrap();
    git(root, &["mv", "crates/pkg-a/file.txt", "crates/pkg-b/file.txt"]);
    git(root, &["commit", "-q", "-m", "refactor: move out of pkg-a"]);

    assert_eq!(
        summaries(root, None, &["crates/pkg-a"]),
        ["refactor: move out of pkg-a", "feat: add pkg-a"]
    );
    assert_eq!(
        summaries(root, None, &["crates/pkg-b"]),
        ["refactor: move out of pkg-a"]
    );
}

#[test]
fn commits_since_detects_binary_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    commit_file(root, "crates/pkg-a/keep.txt", b"seed\n", "feat: seed");
    commit_file(
        root,
        "crates/pkg-a/blob.bin",
        &[0x00, 0xFF, 0xFE, 0x89, 0x50, 0x00, 0xC0, 0xC1],
        "feat: add binary blob",
    );

    assert_eq!(
        summaries(root, None, &["crates/pkg-a"]),
        ["feat: add binary blob", "feat: seed"]
    );
}

#[test]
fn commits_since_normalizes_crlf_messages() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    std::fs::write(root.join("a.txt"), "x\n").unwrap();
    std::fs::write(root.join("msg"), "fix: CRLF summary\r\n\r\nBody line.\r\nSecond.\r\n").unwrap();
    git(root, &["add", "a.txt"]);
    git(root, &["commit", "-q", "-F", "msg"]);

    let commits = GitAccess::new(root, &SystemGit).commits_since(None, &[]).unwrap();

    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].summary, "fix: CRLF summary");
    assert_eq!(commits[0].body.as_deref(), Some("Body line.\nSecond."));
}

#[test]
fn commits_since_reads_a_detached_head() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    commit_file(root, "a.txt", b"1\n", "feat: first");
    let first = head(root);
    commit_file(root, "b.txt", b"2\n", "feat: second");
    git(root, &["checkout", "-q", first.as_str()]);

    assert_eq!(summaries(root, None, &[]), ["feat: first"]);
}

#[test]
fn resolve_commit_peels_tags_and_misses_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    commit_file(root, "a.txt", b"1\n", "feat: first");
    git(root, &["tag", "-a", "-m", "release", "v1.0.0"]);
    let git_access = GitAccess::new(root, &SystemGit);

    assert_eq!(git_access.resolve_commit("v1.0.0").unwrap(), Some(head(root)));
    assert_eq!(git_access.resolve_commit("missing").unwrap(), None);
    assert_eq!(git_access.head_sha().unwrap(), head(root));
}

#[test]
fn list_tags_filters_with_globset() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    commit_file(root, "a.txt", b"1\n", "feat: first");
    git(root, &["tag", "pkg-a@1.0.0"]);
    git(root, &["tag", "pkg-ab@1.0.0"]);
    let git_access = GitAccess::new(root, &SystemGit);

    let tags: Vec<String> = git_access
        .list_tags(Some("pkg-a@*"))
        .unwrap()
        .into_iter()
        .map(|t| t.as_str().to_string())
        .collect();
    assert_eq!(tags, ["pkg-a@1.0.0"]);
    assert!(matches!(
        git_access.list_tags(Some("pkg-a@{malformed")),
        Err(VcsError::InvalidGlob { .. })
    ));
}

fn tag_object(root: &Path, name: &str) -> String {
    git(root, &["cat-file", "-p", name])
}

#[test]
fn annotated_tag_records_the_configured_tagger() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    commit_file(root, "a.txt", b"1\n", "feat: first");

    GitAccess::new(root, &SystemGit)
        .create_tag(
            "v9.9.9",
            &head(root),
            Some("release"),
            TagSignPolicy::ForceUnsigned,
            &ApplyPermit::force_for_tests(),
        )
        .unwrap();

    assert!(tag_object(root, "v9.9.9").contains("\ntagger Test <test@example.com> "));
}

/// A release runner's checkout may configure no identity; the tag must still
/// carry a tagger (GitHub rejects taggerless tag objects).
#[test]
fn annotated_tag_falls_back_to_the_target_committer_without_an_identity() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    commit_file(root, "a.txt", b"1\n", "feat: first");
    git(root, &["config", "--unset", "user.email"]);
    git(root, &["config", "--unset", "user.name"]);
    git(root, &["config", "user.useConfigOnly", "true"]);

    GitAccess::new(root, &SystemGit)
        .create_tag(
            "v9.9.10",
            &head(root),
            Some("release"),
            TagSignPolicy::ForceUnsigned,
            &ApplyPermit::force_for_tests(),
        )
        .unwrap();

    assert!(tag_object(root, "v9.9.10").contains("\ntagger Test <test@example.com> "));
}

#[test]
fn create_tag_refuses_an_existing_tag() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    commit_file(root, "a.txt", b"1\n", "feat: first");
    git(root, &["tag", "dup"]);

    let result = GitAccess::new(root, &SystemGit).create_tag(
        "dup",
        &head(root),
        Some("again"),
        TagSignPolicy::ForceUnsigned,
        &ApplyPermit::force_for_tests(),
    );

    assert!(matches!(result, Err(VcsError::Git(_))), "got {result:?}");
}
