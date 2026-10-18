mod common;

use callisto_cli::cli::{AddArgs, GlobalArgs, OutputFormat, VersionArgs};
use callisto_cli::commands;

use common::setup_polyglot_git_repo;

mod inherited_versions {
    use std::fs;
    use std::path::Path;

    use callisto_fixtures::git::{init_repo, run_git, GitRunner};
    use callisto_graph::{commands::check_partial_run, locate::IgnoreWalkLocator, GraphError, Workspace};
    use callisto_model::{atomic::atomic_write, ApplyPermit};

    fn write(root: &Path, path: &str, content: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        atomic_write(&path, content, &ApplyPermit::force_for_tests()).unwrap();
    }

    fn root_manifest(version: &str) -> String {
        format!(
            "[workspace]\nmembers = [\"crates/member\"]\nresolver = \"2\"\n\n\
             [workspace.package]\nversion = \"{version}\"\n",
        )
    }

    fn fixture(pending: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        write(root, "Cargo.toml", &root_manifest("0.1.0"));
        write(
            root,
            "crates/member/Cargo.toml",
            "[package]\nname = \"member\"\nversion.workspace = true\nedition = \"2021\"\n",
        );
        write(root, "crates/member/src/lib.rs", "\n");
        callisto_fixtures::scaffold_callisto(root);
        if pending {
            write(
                root,
                ".changeset/pending.md",
                "---\nmember: patch\n---\n\nFix the member\n",
            );
        }
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial workspace"]);
        dir
    }

    #[test]
    fn inherited_root_version_change_is_refused_with_pending_changesets_or_a_staged_authority() {
        for pending in [true, false] {
            let dir = fixture(pending);
            let root = dir.path();
            let member_before = fs::read(root.join("crates/member/Cargo.toml")).unwrap();
            write(root, "Cargo.toml", &root_manifest("0.1.1"));
            if !pending {
                run_git(root, &["add", "Cargo.toml"]);
            }
            let runner = GitRunner;
            let locator = IgnoreWalkLocator::new(root);
            let ws = Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
            let error = check_partial_run(&ws).unwrap_err();
            match error {
                GraphError::PartialVersionRun {
                    path,
                    disk,
                    index,
                    head,
                } => {
                    assert_eq!(path, Path::new("Cargo.toml"));
                    assert_eq!(disk.render(), "0.1.1");
                    assert_eq!(index, if pending { "0.1.0" } else { "0.1.1" });
                    assert_eq!(head.render(), "0.1.0");
                }
                other => panic!("expected the partial-version diagnostic, got {other:?}"),
            }
            assert_eq!(fs::read(root.join("crates/member/Cargo.toml")).unwrap(), member_before);
        }
    }

    #[test]
    fn a_version_change_in_a_workspace_below_the_root_is_refused_like_one_at_the_root() {
        for pending in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            init_repo(root);
            write(root, "native/Cargo.toml", &root_manifest("0.1.0"));
            write(
                root,
                "native/crates/member/Cargo.toml",
                "[package]\nname = \"member\"\nversion.workspace = true\nedition = \"2021\"\n",
            );
            write(root, "native/crates/member/src/lib.rs", "\n");
            callisto_fixtures::scaffold_callisto(root);
            if pending {
                write(
                    root,
                    ".changeset/pending.md",
                    "---\nmember: patch\n---\n\nFix the member\n",
                );
            }
            run_git(root, &["add", "."]);
            run_git(root, &["commit", "-q", "-m", "initial workspace"]);

            write(root, "native/Cargo.toml", &root_manifest("0.1.1"));
            if !pending {
                run_git(root, &["add", "native/Cargo.toml"]);
            }
            let runner = GitRunner;
            let locator = IgnoreWalkLocator::new(root);
            let ws = Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
            match check_partial_run(&ws).unwrap_err() {
                GraphError::PartialVersionRun {
                    path,
                    disk,
                    index,
                    head,
                } => {
                    assert_eq!(path, Path::new("native/Cargo.toml"));
                    assert_eq!(disk.render(), "0.1.1");
                    assert_eq!(index, if pending { "0.1.0" } else { "0.1.1" });
                    assert_eq!(head.render(), "0.1.0");
                }
                other => panic!("expected the partial-version diagnostic, got {other:?}"),
            }
        }
    }

    /// A package that names its workspace with `package.workspace` and is moved to another one in the working tree is
    /// still partly applied if the workspace it named at `HEAD` has a staged version bump.
    #[test]
    fn a_staged_bump_in_the_workspace_a_package_named_at_head_is_found_after_it_names_another() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        let workspace = |version: &str| {
            format!("[workspace]\nmembers = [\"../pkgs/p\"]\n\n[workspace.package]\nversion = \"{version}\"\n")
        };
        let member = |workspace: &str| {
            format!(
                "[package]\nname = \"p\"\nversion.workspace = true\nworkspace = \"{workspace}\"\nedition = \"2021\"\n"
            )
        };
        write(root, "old/Cargo.toml", &workspace("0.1.0"));
        write(root, "new/Cargo.toml", &workspace("0.1.0"));
        write(root, "pkgs/p/Cargo.toml", &member("../../old"));
        write(root, "pkgs/p/src/lib.rs", "\n");
        callisto_fixtures::scaffold_callisto(root);
        write(root, ".changeset/pending.md", "---\np: patch\n---\n\nFix p\n");
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial workspace"]);

        write(root, "pkgs/p/Cargo.toml", &member("../../new"));
        write(root, "old/Cargo.toml", &workspace("0.1.1"));
        run_git(root, &["add", "old/Cargo.toml"]);

        let runner = GitRunner;
        let locator = IgnoreWalkLocator::new(root);
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
        match check_partial_run(&ws).unwrap_err() {
            GraphError::PartialVersionRun { index, head, .. } => {
                assert_eq!(index, "0.1.1");
                assert_eq!(head.render(), "0.1.0");
            }
            other => panic!("expected the partial-version diagnostic, got {other:?}"),
        }
    }

    #[test]
    fn staged_workspace_metadata_without_an_effective_version_change_is_allowed() {
        let dir = fixture(false);
        let root = dir.path();
        write(
            root,
            "Cargo.toml",
            &format!("{}\n[workspace.metadata]\nchanged = true\n", root_manifest("0.1.0")),
        );
        run_git(root, &["add", "Cargo.toml"]);
        let runner = GitRunner;
        let locator = IgnoreWalkLocator::new(root);
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
        check_partial_run(&ws).expect("metadata alone does not signal an interrupted version run");
    }

    #[test]
    fn staged_inherited_member_metadata_uses_the_unstaged_workspace_version() {
        let dir = fixture(false);
        let root = dir.path();
        write(
            root,
            "crates/member/Cargo.toml",
            "[package]\nname = \"member\"\nversion.workspace = true\nedition = \"2021\"\n\
             [package.metadata.callisto]\nreviewed = true\n",
        );
        run_git(root, &["add", "crates/member/Cargo.toml"]);
        let runner = GitRunner;
        let locator = IgnoreWalkLocator::new(root);
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
        check_partial_run(&ws).expect("member metadata with an inherited version is not a partial bump");
    }

    #[test]
    fn prerelease_exit_does_not_hide_a_staged_stable_version_bump() {
        let dir = fixture(false);
        let root = dir.path();
        write(root, "Cargo.toml", &root_manifest("0.1.1"));
        write(
            root,
            ".changeset/pre.json",
            r#"{"mode":"exit","tag":"beta","initialVersions":{"member":"0.1.0"},"changesets":[]}"#,
        );
        run_git(root, &["add", "Cargo.toml", ".changeset/pre.json"]);
        let runner = GitRunner;
        let locator = IgnoreWalkLocator::new(root);
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
        assert!(
            matches!(check_partial_run(&ws), Err(GraphError::PartialVersionRun { .. })),
            "an exit state must not exempt an applied stable version bump"
        );
    }

    #[test]
    fn staged_manifest_version_is_checked_even_when_worktree_matches_head() {
        let dir = fixture(false);
        let root = dir.path();
        write(root, "Cargo.toml", &root_manifest("0.1.1"));
        run_git(root, &["add", "Cargo.toml"]);
        write(root, "Cargo.toml", &root_manifest("0.1.0"));

        let runner = GitRunner;
        let locator = IgnoreWalkLocator::new(root);
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
        match check_partial_run(&ws).expect_err("a staged version change must survive worktree restoration") {
            GraphError::PartialVersionRun {
                path,
                disk,
                index,
                head,
            } => {
                assert_eq!(path, Path::new("Cargo.toml"));
                assert_eq!(disk.render(), "0.1.0");
                assert_eq!(index, "0.1.1");
                assert_eq!(head.render(), "0.1.0");
            }
            other => panic!("expected the partial-version diagnostic, got {other:?}"),
        }
    }

    #[test]
    fn untracked_changeset_with_an_applied_root_version_is_refused() {
        let dir = fixture(false);
        let root = dir.path();
        write(root, "Cargo.toml", &root_manifest("0.1.1"));
        write(
            root,
            ".changeset/untracked.md",
            "---\nmember: patch\n---\n\nFix the member\n",
        );
        let runner = GitRunner;
        let locator = IgnoreWalkLocator::new(root);
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
        let error = check_partial_run(&ws).expect_err("an untracked input must not permit a second bump");
        assert!(matches!(error, GraphError::PartialVersionRun { .. }), "got {error:?}");
    }
}

/// Two `version` runs with nothing committed between them must refuse the second (E121), not stack a second bump.
#[test]
fn test_version_twice_without_commit_is_refused_as_partial_run() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    callisto_fixtures::scaffold_callisto(&global.cwd);

    commands::add::handle(
        AddArgs {
            packages: vec!["core-crate:patch".to_string()],
            summary: Some("Fix the frobnication bug".to_string()),
        },
        &global,
    )
    .unwrap();

    commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            no_refresh_lockfiles: true,
            strict: false,
            allow_empty_changesets: false,
            emit_decision: None,
        },
        &global,
    )
    .expect("the first version run must succeed and stage its changes");

    let second = commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            no_refresh_lockfiles: true,
            strict: false,
            allow_empty_changesets: false,
            emit_decision: None,
        },
        &global,
    );

    assert!(
        second.is_err(),
        "a second `version` run with nothing committed since the first must be refused, not \
         silently apply a second bump on top of the first uncommitted one"
    );
    let message = second.unwrap_err().to_string();
    assert!(
        message.contains("this looks like an interrupted `version` run") || message.contains("E121"),
        "expected the partial-run (E121) refusal, got: {message}"
    );
}

#[test]
fn decision_write_failure_leaves_staged_evidence_for_a_safe_retry_refusal() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();
    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };
    callisto_fixtures::scaffold_callisto(root);
    std::fs::write(root.join("decision-parent"), "not a directory").unwrap();
    commands::add::handle(
        AddArgs {
            packages: vec!["core-crate:patch".to_string()],
            summary: Some("Retain recovery evidence".to_string()),
        },
        &global,
    )
    .unwrap();

    let first = commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            no_refresh_lockfiles: true,
            strict: false,
            allow_empty_changesets: false,
            emit_decision: Some("decision-parent/release-decision.json".into()),
        },
        &global,
    );
    assert!(first.is_err(), "writing into a missing directory must fail after apply");
    let staged = std::process::Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(root)
        .status()
        .unwrap();
    assert_eq!(
        staged.code(),
        Some(1),
        "the applied version must remain staged as recovery evidence"
    );

    let retry = commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            no_refresh_lockfiles: true,
            strict: false,
            allow_empty_changesets: false,
            emit_decision: None,
        },
        &global,
    );
    assert!(retry.is_err(), "a fresh command must refuse to stack a second bump");
    let retry_error = retry.unwrap_err().to_string();
    assert!(
        retry_error.contains("this looks like an interrupted `version` run"),
        "expected the recovery refusal, got: {retry_error}"
    );
}
