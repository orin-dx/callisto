mod common;

use std::fs;
use std::process::Command;

use callisto_cli::cli::{AddArgs, GlobalArgs, OutputFormat, PreArgs, VersionArgs};
use callisto_cli::commands;

use common::setup_polyglot_git_repo;

#[test]
fn test_pre_mode_blackbox_lifecycle() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    // 1. Enter pre-release mode with tag 'beta'
    let enter_res = commands::pre::handle(
        PreArgs::Enter {
            tag: "beta".to_string(),
        },
        &global,
    );
    assert!(enter_res.is_ok());
    assert!(root.join(".changeset/pre.json").exists());

    // 2. Add changeset for minor bump
    let add_res = commands::add::handle(
        AddArgs {
            packages: vec!["core-crate:minor".to_string()],
            summary: Some("Beta feature".to_string()),
        },
        &global,
    );
    assert!(add_res.is_ok());

    // 3. Version in pre-release mode -> should produce 0.2.0-beta.0
    let version_res = commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            no_refresh_lockfiles: true,
            strict: false,
            allow_empty_changesets: false,
            emit_decision: None,
        },
        &global,
    );
    assert!(version_res.is_ok());

    let updated_cargo = fs::read_to_string(root.join("crates/core/Cargo.toml")).unwrap();
    assert!(updated_cargo.contains("0.2.0-beta.0"));

    // 4. Exit pre-release mode
    let exit_res = commands::pre::handle(PreArgs::Exit, &global);
    assert!(exit_res.is_ok());

    // 5. Final versioning after exit -> should finalize to 0.2.0
    let final_version_res = commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            no_refresh_lockfiles: true,
            strict: false,
            allow_empty_changesets: true,
            emit_decision: None,
        },
        &global,
    );
    assert!(final_version_res.is_ok());

    let final_cargo = fs::read_to_string(root.join("crates/core/Cargo.toml")).unwrap();
    assert!(
        final_cargo.contains("version = \"0.2.0\""),
        "Expected final version 0.2.0 in Cargo.toml, got:\n{final_cargo}"
    );
}

#[cfg(unix)]
#[test]
fn failed_pre_state_deletion_fails_closed_on_a_fresh_version_retry() {
    use std::os::unix::fs::PermissionsExt;

    let dir = setup_polyglot_git_repo();
    let root = dir.path();
    let pre_json = root.join(".changeset/pre.json");

    let enter = run_callisto_cli(root, &["pre", "enter", "beta"], None);
    assert!(
        enter.status.success(),
        "pre enter failed: {}",
        String::from_utf8_lossy(&enter.stderr)
    );
    Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "enter prerelease mode"])
        .current_dir(root)
        .status()
        .unwrap();

    let add = run_callisto_cli(
        root,
        &[
            "add",
            "--package",
            "core-crate:minor",
            "--summary",
            "Exercise failed exit finalization",
        ],
        None,
    );
    assert!(
        add.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let prerelease = run_callisto_cli(root, &["version", "--no-refresh-lockfiles"], None);
    assert!(
        prerelease.status.success(),
        "prerelease version failed: {}",
        String::from_utf8_lossy(&prerelease.stderr)
    );
    let exit = run_callisto_cli(root, &["pre", "exit"], None);
    assert!(
        exit.status.success(),
        "pre exit failed: {}",
        String::from_utf8_lossy(&exit.stderr)
    );

    let temp = tempfile::tempdir().unwrap();
    let fake_bin = temp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|entry| entry.join("git"))
        .find(|path| path.is_file())
        .expect("find the real Git executable");
    let fake_git = fake_bin.join("git");
    fs::write(
        &fake_git,
        r##"#!/bin/sh
printf '%s\n' "$*" >> "$CALLISTO_TEST_GIT_TRACE"
if [ "$1" = "rm" ] && [ "$2" = "--cached" ]; then
  for path do
    if [ "$path" = ".changeset/pre.json" ]; then
      printf 'forced pre-state staging failure\n' >&2
      exit 1
    fi
  done
fi
exec "$CALLISTO_TEST_REAL_GIT" "$@"
"##,
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake_git).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_git, permissions).unwrap();
    let mut path_entries = vec![fake_bin];
    path_entries.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let path = std::env::join_paths(path_entries).unwrap();
    let git_trace = temp.path().join("git-trace");

    let failed_exit_version = run_callisto_cli(
        root,
        &["version", "--no-refresh-lockfiles"],
        Some((path.as_os_str(), &real_git, &git_trace)),
    );
    assert!(
        !failed_exit_version.status.success(),
        "version must report the forced git rm failure; trace: {}; stdout: {}; stderr: {}",
        fs::read_to_string(&git_trace).unwrap_or_default(),
        String::from_utf8_lossy(&failed_exit_version.stdout),
        String::from_utf8_lossy(&failed_exit_version.stderr)
    );
    let git_calls = fs::read_to_string(&git_trace).unwrap();
    assert!(
        git_calls
            .lines()
            .any(|call| call.starts_with("rm --cached") && call.contains(".changeset/pre.json")),
        "the fixture must fail the pre-state deletion command, got: {git_calls}"
    );
    assert!(
        String::from_utf8_lossy(&failed_exit_version.stderr).contains("forced pre-state staging failure"),
        "the first command must surface the injected Git failure: {}",
        String::from_utf8_lossy(&failed_exit_version.stderr)
    );
    assert!(pre_json.exists(), "failed staging must retain pre.json on disk");
    assert!(
        fs::read_to_string(root.join("crates/core/Cargo.toml"))
            .unwrap()
            .contains("version = \"0.2.0\""),
        "stable outputs should already be written before deletion staging fails"
    );

    let retry = run_callisto_cli(root, &["version", "--no-refresh-lockfiles"], None);
    assert!(!retry.status.success(), "a fresh retry must fail closed with E121");
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&retry.stdout),
        String::from_utf8_lossy(&retry.stderr)
    )
    .split_whitespace()
    .collect::<Vec<_>>()
    .join(" ");
    assert!(diagnostic.contains("E121"), "expected E121, got: {diagnostic}");
    assert!(
        diagnostic.contains("git diff --cached"),
        "recovery help must inspect staged changes: {diagnostic}"
    );
    assert!(
        diagnostic.contains("git rm -- <path>"),
        "recovery help must name manual input removal: {diagnostic}"
    );
    assert!(
        pre_json.exists(),
        "the refused retry must not delete the retained input"
    );
}

fn run_callisto_cli(
    root: &std::path::Path,
    args: &[&str],
    git: Option<(&std::ffi::OsStr, &std::path::Path, &std::path::Path)>,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_callisto"));
    command
        .args(["--cwd", root.to_str().unwrap(), "--format", "text"])
        .args(args);
    if let Some((path, real_git, trace)) = git {
        command
            .env("PATH", path)
            .env("CALLISTO_TEST_REAL_GIT", real_git)
            .env("CALLISTO_TEST_GIT_TRACE", trace);
    }
    command.output().unwrap()
}

/// Regression test: if `.changeset/pre.json` exists but is malformed (truncated,
/// corrupted, etc.), `callisto version` must return `Err`, not silently fall back
/// to normal release mode and consume changesets as if no pre-release was active.
///
/// Previously, `.ok()` swallowed all parse errors and returned `None`, causing
/// the version command to behave as if no pre.json existed at all.
#[test]
fn test_malformed_pre_json_returns_error_not_silent_normal_release() {
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
            summary: Some("Test patch".to_string()),
        },
        &global,
    )
    .unwrap();

    let changeset_dir = root.join(".changeset");
    fs::write(changeset_dir.join("pre.json"), r#"{"mode": "p"#).unwrap();

    let version_res = commands::version::handle(
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
        version_res.is_err(),
        "expected Err when .changeset/pre.json is malformed, but version command returned Ok"
    );

    let cargo_content = fs::read_to_string(root.join("crates/core/Cargo.toml")).unwrap();
    assert!(
        cargo_content.contains("version = \"0.1.0\""),
        "Cargo.toml must not be modified when pre.json is malformed, but got:\n{cargo_content}"
    );
}

/// Bug 1: `pre enter` must refuse to overwrite an existing pre.json.
///
/// Running `pre enter` a second time while the workspace is already in
/// pre-release mode should return an error, not silently overwrite the file.
/// The check must fire in both the real-write and dry-run paths.
#[test]
fn test_pre_enter_rejects_if_already_in_pre_mode() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Text,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    let first = commands::pre::handle(
        PreArgs::Enter {
            tag: "alpha".to_string(),
        },
        &global,
    );
    assert!(first.is_ok(), "first pre enter must succeed: {first:?}");
    assert!(
        root.join(".changeset/pre.json").exists(),
        "pre.json must exist after first enter"
    );

    let second = commands::pre::handle(
        PreArgs::Enter {
            tag: "beta".to_string(),
        },
        &global,
    );
    assert!(
        second.is_err(),
        "second pre enter must fail when pre.json already exists, but got Ok"
    );

    let content = fs::read_to_string(root.join(".changeset/pre.json")).unwrap();
    assert!(
        content.contains("alpha"),
        "pre.json must still contain 'alpha' after rejected second enter: {content}"
    );
    assert!(
        !content.contains("beta"),
        "pre.json must NOT contain 'beta' after rejected second enter: {content}"
    );

    let global_dry = GlobalArgs {
        dry_run: true,
        ..global.clone()
    };
    let dry = commands::pre::handle(PreArgs::Enter { tag: "rc".to_string() }, &global_dry);
    assert!(
        dry.is_err(),
        "dry-run pre enter must also fail when pre.json already exists on disk"
    );
}

/// Bug 2: `pre exit` must refuse to run when already in Exit mode (double-exit).
#[test]
fn test_pre_exit_rejects_double_exit() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Text,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::pre::handle(
        PreArgs::Enter {
            tag: "alpha".to_string(),
        },
        &global,
    )
    .expect("pre enter must succeed");

    let first_exit = commands::pre::handle(PreArgs::Exit, &global);
    assert!(first_exit.is_ok(), "first pre exit must succeed: {first_exit:?}");

    let second_exit = commands::pre::handle(PreArgs::Exit, &global);
    assert!(
        second_exit.is_err(),
        "second pre exit must fail when already in Exit mode, but got Ok"
    );
}

/// Bug 3: `pre enter` must reject an empty tag string.
///
/// An empty tag produces `"1.0.0-.0"` during versioning, which is not valid semver.
#[test]
fn test_pre_enter_rejects_empty_tag() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Text,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    let result = commands::pre::handle(PreArgs::Enter { tag: "".to_string() }, &global);
    assert!(
        result.is_err(),
        "pre enter with empty tag must return an error, but got Ok"
    );

    assert!(
        !root.join(".changeset/pre.json").exists(),
        "pre.json must not be created when the tag is empty"
    );
}

/// Bug 4: `pre enter` must stage `.changeset/pre.json` via `git add`.
///
/// After a successful `pre enter`, `git status --porcelain` should show
/// `.changeset/pre.json` as staged (index status 'A'), not as untracked ('??').
#[test]
fn test_pre_enter_stages_pre_json_via_git_add() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Text,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::pre::handle(
        PreArgs::Enter {
            tag: "alpha".to_string(),
        },
        &global,
    )
    .expect("pre enter must succeed");

    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .expect("git status must run");
    let stdout = String::from_utf8_lossy(&output.stdout);

    let is_staged = stdout
        .lines()
        .any(|line| (line.starts_with('A') || line.starts_with("AM")) && line.contains("pre.json"));
    let is_untracked = stdout.lines().any(|l| l.starts_with("??") && l.contains("pre.json"));

    assert!(
        is_staged,
        "pre.json must be staged (git add) after pre enter, but git status shows:\n{stdout}"
    );
    assert!(
        !is_untracked,
        "pre.json must NOT be untracked after pre enter, but git status shows:\n{stdout}"
    );
}

/// `pre exit` must stage `pre.json` the same way `pre enter` does.
#[test]
fn test_pre_exit_stages_pre_json_via_git_add() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Text,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::pre::handle(
        PreArgs::Enter {
            tag: "alpha".to_string(),
        },
        &global,
    )
    .expect("pre enter must succeed");
    Command::new("git")
        .args(["commit", "-m", "enter pre mode"])
        .current_dir(root)
        .status()
        .unwrap();

    commands::pre::handle(PreArgs::Exit, &global).expect("pre exit must succeed");

    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .expect("git status must run");
    let stdout = String::from_utf8_lossy(&output.stdout);

    let is_staged = stdout
        .lines()
        .any(|line| (line.starts_with('M') || line.starts_with("AM")) && line.contains("pre.json"));

    assert!(
        is_staged,
        "pre.json must be staged (git add) after pre exit, but git status shows:\n{stdout}"
    );
}

/// Running `version` twice in pre mode with no new changeset must not duplicate the changelog entry.
#[test]
fn test_pre_mode_rerun_does_not_duplicate_changelog_entry() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::pre::handle(
        PreArgs::Enter {
            tag: "beta".to_string(),
        },
        &global,
    )
    .expect("pre enter must succeed");

    commands::add::handle(
        AddArgs {
            packages: vec!["core-crate:minor".to_string()],
            summary: Some("Beta feature".to_string()),
        },
        &global,
    )
    .expect("add must succeed");

    for _ in 0..2 {
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
        .expect("version must succeed in pre mode");
    }

    let pre_json = fs::read_to_string(root.join(".changeset/pre.json")).unwrap();
    let pre_state = callisto_model::format::parse_pre_json(&pre_json).unwrap();
    assert_eq!(
        pre_state.changesets.len(),
        1,
        "pre.json must record exactly one changeset id (the one `add` created), not zero and not \
         duplicated across two `version` runs; got: {:?}",
        pre_state.changesets
    );

    let changelog = fs::read_to_string(root.join("crates/core/CHANGELOG.md")).unwrap();
    let occurrences = changelog.matches("Beta feature").count();
    assert_eq!(
        occurrences, 1,
        "the changeset's changelog line must appear exactly once even after two `version` runs \
         in pre mode, got {occurrences} occurrences in:\n{changelog}"
    );

    // Counts total bullet entries, not just a substring match, so a second synthesized entry can't hide alongside it.
    let total_entries = changelog.lines().filter(|l| l.starts_with("- ")).count();
    assert_eq!(
        total_entries, 1,
        "the changelog must have exactly one entry after two `version` runs in pre mode with no \
         commit between them, got {total_entries} in:\n{changelog}"
    );

    // The changeset file must remain on disk -- pre mode never deletes it until `pre exit`'s consuming run.
    let cs_dir = root.join(".changeset");
    let remaining_md: Vec<_> = fs::read_dir(&cs_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("md"))
        .filter(|e| e.file_name() != "README.md")
        .collect();
    assert!(
        !remaining_md.is_empty(),
        "the pre-mode changeset file must remain on disk until `pre exit`"
    );
}
