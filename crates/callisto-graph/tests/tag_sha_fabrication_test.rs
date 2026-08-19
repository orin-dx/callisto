//! Regression tests for tag.rs's per-release-tag sha fabrication fix
//! (AC-06, AC-06b, AC-06c, AC-06d, AC-07, AC-08). This file is built up
//! across three tasks (T3: apply path, T4: dry-run path); each task adds
//! its own #[test] functions to the shared fixtures below.

use std::path::Path;

use callisto_graph::commands::{create_tags_with_options, TagOptions};
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::{GraphError, Workspace};
use callisto_model::{
    ApplyPermit, CommandError, CommandOutput, CommandRunner, CommitSha, PackageId, PublishPlan,
    ReleaseEntry, TagName, SCHEMA_VERSION,
};

fn write_minimal_workspace(root: &Path) {
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    let pkg_dir = root.join("crates/my-app");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    std::fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
}

enum RevParse {
    Sha(String),
    NotFound,
    CommandErr,
}

/// A `CommandRunner` double answering `git tag --list` (whether the
/// per-release tag exists) and `git rev-parse --verify --quiet <tag>^{commit}`
/// (the tag's real current target, or a failure) so the apply and dry-run
/// paths' existing-tag-sha-resolution logic can be tested without a real
/// diverging git repository.
struct StubTagRunner {
    tag_exists: bool,
    tag_name: String,
    rev_parse: RevParse,
}

impl CommandRunner for StubTagRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        _cwd: &Path,
    ) -> Result<CommandOutput, CommandError> {
        assert_eq!(program, "git", "only `git` should be shelled out to here");
        match args {
            ["tag", "--list"] => Ok(CommandOutput {
                exit_code: Some(0),
                stdout: if self.tag_exists {
                    format!("{}\n", self.tag_name)
                } else {
                    String::new()
                },
                stderr: String::new(),
            }),
            ["rev-parse", "--verify", "--quiet", rev] => {
                let expected = format!("{}^{{commit}}", self.tag_name);
                assert_eq!(*rev, expected.as_str());
                match &self.rev_parse {
                    RevParse::Sha(s) => Ok(CommandOutput {
                        exit_code: Some(0),
                        stdout: format!("{s}\n"),
                        stderr: String::new(),
                    }),
                    RevParse::NotFound => Ok(CommandOutput {
                        exit_code: Some(1),
                        stdout: String::new(),
                        stderr: String::new(),
                    }),
                    RevParse::CommandErr => Err(CommandError::NotFound {
                        program: "git".to_string(),
                    }),
                }
            }
            other => panic!("unexpected git invocation: {other:?}"),
        }
    }
}

fn assert_non_repo(root: &Path) {
    assert!(
        callisto_vcs::GitRepository::discover(root).is_err(),
        "test fixture must not be discoverable as a Git repo"
    );
}

fn release_entry(tag_name: &str, sha: &str) -> ReleaseEntry {
    ReleaseEntry {
        package: PackageId::parse("pkg").unwrap(),
        tag_name: TagName(tag_name.to_string()),
        sha: CommitSha::parse(sha).unwrap(),
        changelog_section: None,
    }
}

fn plan_with_release(entry: ReleaseEntry) -> PublishPlan {
    PublishPlan {
        schema_version: SCHEMA_VERSION,
        rust_crates: vec![],
        npm_main_packages: vec![],
        npm_platform_packages: vec![],
        pypi_packages: vec![],
        releases: vec![entry],
        diagnostics: vec![],
    }
}

// ---- T3: apply path (permit Some) ----

#[test]
fn apply_mode_reports_real_target_when_tag_exists_and_differs_from_release_sha() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_minimal_workspace(root);
    assert_non_repo(root);

    let real_sha = "a".repeat(40);
    let requested_sha = "b".repeat(40);
    let runner = StubTagRunner {
        tag_exists: true,
        tag_name: "pkg@1.0.0".to_string(),
        rev_parse: RevParse::Sha(real_sha.clone()),
    };
    let locator = IgnoreWalkLocator::new(root);
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
    let plan = plan_with_release(release_entry("pkg@1.0.0", &requested_sha));
    let permit = ApplyPermit::force_for_tests();

    let report = create_tags_with_options(&ws, &plan, &TagOptions::default(), Some(&permit))
        .expect("create_tags_with_options must succeed");

    assert_eq!(
        report.created_tags[0].sha.as_str(),
        real_sha,
        "apply mode must report the tag's real current target, not release.sha, when the tag \
         already exists at a different commit"
    );
}

#[test]
fn apply_mode_fails_fast_when_resolve_commit_returns_ok_none_for_existing_tag() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_minimal_workspace(root);
    assert_non_repo(root);

    let runner = StubTagRunner {
        tag_exists: true,
        tag_name: "pkg@1.0.0".to_string(),
        rev_parse: RevParse::NotFound,
    };
    let locator = IgnoreWalkLocator::new(root);
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
    let plan = plan_with_release(release_entry("pkg@1.0.0", &"b".repeat(40)));
    let permit = ApplyPermit::force_for_tests();

    let result = create_tags_with_options(&ws, &plan, &TagOptions::default(), Some(&permit));

    assert!(
        matches!(result, Err(GraphError::Vcs(_))),
        "must return Err(GraphError::Vcs) when resolve_commit returns Ok(None) for an \
         already-existing tag, not fabricate release.sha; got {result:?}"
    );
}

#[test]
fn apply_mode_fails_fast_when_resolve_commit_errors_for_existing_tag() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_minimal_workspace(root);
    assert_non_repo(root);

    let runner = StubTagRunner {
        tag_exists: true,
        tag_name: "pkg@1.0.0".to_string(),
        rev_parse: RevParse::CommandErr,
    };
    let locator = IgnoreWalkLocator::new(root);
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
    let plan = plan_with_release(release_entry("pkg@1.0.0", &"b".repeat(40)));
    let permit = ApplyPermit::force_for_tests();

    let result = create_tags_with_options(&ws, &plan, &TagOptions::default(), Some(&permit));

    assert!(
        matches!(result, Err(GraphError::Vcs(_))),
        "must return Err(GraphError::Vcs) when resolve_commit itself errors for an \
         already-existing tag, not fabricate release.sha; got {result:?}"
    );
}

// ---- T4: dry-run path (permit None) ----

#[test]
fn dry_run_mode_reports_real_target_when_tag_exists_and_differs_from_release_sha() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_minimal_workspace(root);
    assert_non_repo(root);

    let real_sha = "a".repeat(40);
    let requested_sha = "b".repeat(40);
    let runner = StubTagRunner {
        tag_exists: true,
        tag_name: "pkg@1.0.0".to_string(),
        rev_parse: RevParse::Sha(real_sha.clone()),
    };
    let locator = IgnoreWalkLocator::new(root);
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
    let plan = plan_with_release(release_entry("pkg@1.0.0", &requested_sha));

    let report = create_tags_with_options(&ws, &plan, &TagOptions::default(), None)
        .expect("create_tags_with_options (dry-run) must succeed");

    assert_eq!(
        report.created_tags[0].sha.as_str(),
        real_sha,
        "dry-run mode must preview the tag's real current target, not release.sha, when the \
         tag already exists at a different commit"
    );
    // StubTagRunner panics on any git invocation other than `tag --list` and
    // `rev-parse --verify --quiet ...` -- reaching this point without a panic
    // proves no write op (e.g. `git tag -a`) was ever attempted.
}

#[test]
fn dry_run_mode_fails_fast_when_resolve_commit_returns_ok_none_for_existing_tag() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_minimal_workspace(root);
    assert_non_repo(root);

    let runner = StubTagRunner {
        tag_exists: true,
        tag_name: "pkg@1.0.0".to_string(),
        rev_parse: RevParse::NotFound,
    };
    let locator = IgnoreWalkLocator::new(root);
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
    let plan = plan_with_release(release_entry("pkg@1.0.0", &"b".repeat(40)));

    let result = create_tags_with_options(&ws, &plan, &TagOptions::default(), None);

    assert!(
        matches!(result, Err(GraphError::Vcs(_))),
        "dry-run must return Err(GraphError::Vcs) when resolve_commit returns Ok(None) for an \
         already-existing tag, not fabricate release.sha; got {result:?}"
    );
}

#[test]
fn dry_run_mode_fails_fast_when_resolve_commit_errors_for_existing_tag() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_minimal_workspace(root);
    assert_non_repo(root);

    let runner = StubTagRunner {
        tag_exists: true,
        tag_name: "pkg@1.0.0".to_string(),
        rev_parse: RevParse::CommandErr,
    };
    let locator = IgnoreWalkLocator::new(root);
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
    let plan = plan_with_release(release_entry("pkg@1.0.0", &"b".repeat(40)));

    let result = create_tags_with_options(&ws, &plan, &TagOptions::default(), None);

    assert!(
        matches!(result, Err(GraphError::Vcs(_))),
        "dry-run must return Err(GraphError::Vcs) when resolve_commit itself errors for an \
         already-existing tag, not fabricate release.sha; got {result:?}"
    );
}

// ---- T5: regression proofs (real git repo, no stubbing) ----

struct PanicRunner;
impl CommandRunner for PanicRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        _cwd: &Path,
    ) -> Result<CommandOutput, CommandError> {
        panic!("unexpected CommandRunner invocation ({program} {args:?}) -- a real .git repo is present, so GitAccess must use native gix, never the shell fallback");
    }
}

fn init_git_repo(dir: &Path) {
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git should run");
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test"]);
    // Force local, unsigned lightweight tags/commits regardless of the
    // developer's global git config (e.g. tag.gpgsign=true), so the
    // fixture's plain `git tag <name> <sha>` never blocks on a missing
    // annotation message or signing key.
    run(&["config", "commit.gpgsign", "false"]);
    run(&["config", "tag.gpgsign", "false"]);
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "init"]);
}

fn git_head_sha(dir: &Path) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn git_tag_exists_at(dir: &Path, tag: &str, expected_sha: &str) -> bool {
    let out = std::process::Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{tag}^{{commit}}"),
        ])
        .current_dir(dir)
        .output()
        .unwrap();
    out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == expected_sha
}

#[test]
fn tag_not_existing_reports_release_sha_in_both_modes_and_apply_creates_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_minimal_workspace(root);
    init_git_repo(root);
    let head = git_head_sha(root);

    let runner = PanicRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
    let plan = plan_with_release(release_entry("pkg@1.0.0", &head));

    // Dry-run first, while the tag genuinely does not exist yet.
    let dry_report = create_tags_with_options(&ws, &plan, &TagOptions::default(), None)
        .expect("dry-run must succeed");
    assert_eq!(
        dry_report.created_tags[0].sha.as_str(),
        head,
        "AC-07 dry-run"
    );
    assert!(
        !git_tag_exists_at(root, "pkg@1.0.0", &head),
        "dry-run must not create the tag"
    );

    // Apply: tag still does not exist -> gets created at release.sha.
    let permit = ApplyPermit::force_for_tests();
    let apply_report = create_tags_with_options(&ws, &plan, &TagOptions::default(), Some(&permit))
        .expect("apply must succeed");
    assert_eq!(
        apply_report.created_tags[0].sha.as_str(),
        head,
        "AC-07 apply"
    );
    assert!(
        git_tag_exists_at(root, "pkg@1.0.0", &head),
        "apply must create the tag at release.sha"
    );
}

#[test]
fn tag_existing_at_release_sha_reports_release_sha_with_no_error_in_both_modes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_minimal_workspace(root);
    init_git_repo(root);
    let head = git_head_sha(root);

    // Pre-create the tag directly via git, at exactly the sha the plan will request.
    let status = std::process::Command::new("git")
        .args(["tag", "pkg@1.0.0", &head])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success());

    let runner = PanicRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
    let plan = plan_with_release(release_entry("pkg@1.0.0", &head));

    let permit = ApplyPermit::force_for_tests();
    let apply_report = create_tags_with_options(&ws, &plan, &TagOptions::default(), Some(&permit))
        .expect("idempotent apply rerun must succeed with no error");
    assert_eq!(
        apply_report.created_tags[0].sha.as_str(),
        head,
        "AC-08 apply"
    );

    let dry_report = create_tags_with_options(&ws, &plan, &TagOptions::default(), None)
        .expect("dry-run against already-tagged state must succeed with no error");
    assert_eq!(
        dry_report.created_tags[0].sha.as_str(),
        head,
        "AC-08 dry-run"
    );
}
