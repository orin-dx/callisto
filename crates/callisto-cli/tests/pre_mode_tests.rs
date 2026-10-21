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

mod python_baselines {
    use std::fs;
    use std::path::Path;
    use std::process::{Command, Output};

    use callisto_model::atomic::atomic_write;
    use callisto_model::ApplyPermit;
    use serde_json::{json, Value};

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let permit = ApplyPermit::granted_unless_dry_run(false).unwrap();
        atomic_write(&path, text, &permit).unwrap();
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git").args(args).current_dir(root).output().unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn commit(root: &Path) {
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "fixture"]);
    }

    fn workspace(shared_name: bool, dir: &str) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        callisto_fixtures::git::init_repo(root);
        write(
            root,
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/core\"]\nresolver = \"2\"\n",
        );
        let cargo_name = if shared_name { "shared" } else { "rust-lib" };
        write(
            root,
            "crates/core/Cargo.toml",
            &format!("[package]\nname = \"{cargo_name}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n"),
        );
        write(root, "crates/core/src/lib.rs", "\n");
        write(root, "package.json", r#"{"private":true,"workspaces":["packages/*"]}"#);
        let npm_name = if shared_name { "shared" } else { "web-lib" };
        write(
            root,
            "packages/web/package.json",
            &json!({"name": npm_name, "version": "2.0.0"}).to_string(),
        );
        let python_name = if shared_name { "shared" } else { "py-lib" };
        write(
            root,
            "packages/python/pyproject.toml",
            &format!("[project]\nname = \"{python_name}\"\nversion = \"1.2.3\"\n"),
        );
        write(root, "callisto.toml", &format!("[changesets]\ndir = \"{dir}\"\n"));
        commit(root);
        temp
    }

    fn callisto(root: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_callisto"))
            .args(["--cwd", root.to_str().unwrap(), "--format", "json"])
            .args(args)
            .output()
            .unwrap()
    }

    fn run(root: &Path, args: &[&str]) {
        let output = callisto(root, args);
        assert!(
            output.status.success(),
            "callisto {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn version(root: &Path) {
        run(root, &["version", "--no-refresh-lockfiles"]);
    }

    fn pre_json(root: &Path, dir: &str) -> Value {
        serde_json::from_slice(&fs::read(root.join(dir).join("pre.json")).unwrap()).unwrap()
    }

    fn manifest_version(root: &Path, path: &str, format: callisto_model::ManifestFormat) -> String {
        let text = fs::read_to_string(root.join(path)).unwrap();
        let identity = callisto_manifests::read_identity(format, &text, &root.join(path)).unwrap();
        match identity.version.unwrap() {
            callisto_manifests::VersionSource::Literal(version) => version,
            source => panic!("unexpected version source: {source:?}"),
        }
    }

    fn assert_versions(root: &Path, python: &str, cargo: &str, npm: &str) {
        assert_eq!(
            manifest_version(
                root,
                "packages/python/pyproject.toml",
                callisto_model::ManifestFormat::PyprojectToml
            ),
            python
        );
        assert_eq!(
            manifest_version(
                root,
                "crates/core/Cargo.toml",
                callisto_model::ManifestFormat::CargoToml
            ),
            cargo
        );
        let package: Value =
            serde_json::from_slice(&fs::read(root.join("packages/web/package.json")).unwrap()).unwrap();
        assert_eq!(package["version"], npm);
    }

    #[test]
    fn stable_python_and_semver_siblings_complete_repeated_prerelease_cycles() {
        for (dir, tag, suffix) in [(".changeset", "beta", "b"), ("release-notes/pending", "next", ".dev")] {
            let temp = workspace(false, dir);
            let root = temp.path();
            run(root, &["pre", "enter", tag]);
            let initial = pre_json(root, dir);
            assert_eq!(
                initial["initialVersions"],
                json!({"py-lib":"1.2.3","rust-lib":"1.0.0","web-lib":"2.0.0"})
            );
            assert_eq!(initial.as_object().unwrap().len(), 4);
            run(
                root,
                &[
                    "add",
                    "--package",
                    "pypi/py-lib:minor",
                    "--package",
                    "cargo/rust-lib:patch",
                    "--package",
                    "npm/web-lib:patch",
                    "--summary",
                    "Native prerelease feature",
                ],
            );
            for counter in 0..2 {
                version(root);
                assert_versions(
                    root,
                    &format!("1.3.0{suffix}{counter}"),
                    &format!("1.0.1-{tag}.{counter}"),
                    &format!("2.0.1-{tag}.{counter}"),
                );
                let state = pre_json(root, dir);
                assert_eq!(state["initialVersions"], initial["initialVersions"]);
                assert_eq!(state["changesets"].as_array().unwrap().len(), 1);
                let id = state["changesets"][0].as_str().unwrap();
                assert!(root.join(dir).join(format!("{id}.md")).exists());
            }
            let changelog = fs::read_to_string(root.join("packages/python/CHANGELOG.md")).unwrap();
            assert_eq!(changelog.matches("Native prerelease feature").count(), 1);
            run(root, &["pre", "exit"]);
            assert_eq!(pre_json(root, dir)["mode"], "exit");
            assert_eq!(pre_json(root, dir)["initialVersions"], initial["initialVersions"]);
            version(root);
            assert_versions(root, "1.3.0", "1.0.1", "2.0.1");
            assert!(!root.join(dir).join("pre.json").exists());
            assert!(!root.join(".changeset/pre.json").exists());
            assert!(!fs::read_dir(root.join(dir)).unwrap().any(|entry| {
                let entry = entry.unwrap();
                entry.path().extension().is_some_and(|extension| extension == "md") && entry.file_name() != "README.md"
            }));
        }
    }

    #[test]
    fn qualified_python_baselines_resolve_to_an_unpromoted_owner() {
        let temp = workspace(false, ".changeset");
        let root = temp.path();
        run(root, &["pre", "enter", "beta"]);
        let mut state = pre_json(root, ".changeset");
        let baseline = state["initialVersions"]
            .as_object_mut()
            .unwrap()
            .remove("py-lib")
            .unwrap();
        state["initialVersions"]["pypi/py-lib"] = baseline;
        write(root, ".changeset/pre.json", &state.to_string());
        run(
            root,
            &["add", "--package", "py-lib:minor", "--summary", "Qualified baseline"],
        );
        version(root);
        version(root);
        assert_versions(root, "1.3.0b1", "1.0.0", "2.0.0");
        let state = pre_json(root, ".changeset");
        assert_eq!(state["initialVersions"]["py-lib"], "1.2.3");
        assert!(state["initialVersions"].get("pypi/py-lib").is_none());
    }

    #[test]
    fn same_named_python_cargo_and_npm_packages_keep_separate_native_baselines() {
        let temp = workspace(true, ".changeset");
        let root = temp.path();
        run(root, &["pre", "enter", "beta"]);
        let initial = pre_json(root, ".changeset");
        assert_eq!(
            initial["initialVersions"],
            json!({"pypi/shared":"1.2.3","cargo/shared":"1.0.0","npm/shared":"2.0.0"})
        );
        run(
            root,
            &[
                "add",
                "--package",
                "pypi/shared:minor",
                "--package",
                "cargo/shared:patch",
                "--package",
                "npm/shared:patch",
                "--summary",
                "Qualified native cycle",
            ],
        );
        version(root);
        version(root);
        assert_versions(root, "1.3.0b1", "1.0.1-beta.1", "2.0.1-beta.1");
        assert_eq!(
            pre_json(root, ".changeset")["initialVersions"],
            initial["initialVersions"]
        );
        run(root, &["pre", "exit"]);
        version(root);
        assert_versions(root, "1.3.0", "1.0.1", "2.0.1");
    }

    #[test]
    fn historical_bare_python_baseline_follows_its_owner_after_promotion() {
        let dir = "release-notes/pending";
        let temp = workspace(false, dir);
        let root = temp.path();
        run(root, &["pre", "enter", "beta"]);
        commit(root);
        write(
            root,
            &format!("{dir}/first.md"),
            "---\npypi/py-lib: minor\n---\n\nHistorical Python baseline.\n",
        );
        version(root);
        commit(root);
        write(
            root,
            "packages/sibling/package.json",
            r#"{"name":"py-lib","version":"3.0.0"}"#,
        );
        write(
            root,
            &format!("{dir}/extra.md"),
            "---\npypi/py-lib: patch\n---\n\nKeep the historical anchor.\n",
        );
        let mut state = pre_json(root, dir);
        state["initialVersions"]["removed-package"] = json!("3.4.5");
        write(root, &format!("{dir}/pre.json"), &state.to_string());
        version(root);
        assert_versions(root, "1.3.0b1", "1.0.0", "2.0.0");
        let state = pre_json(root, dir);
        assert_eq!(state["initialVersions"]["pypi/py-lib"], "1.2.3");
        assert!(state["initialVersions"].get("py-lib").is_none());
        assert!(state["initialVersions"].get("npm/py-lib").is_none());
        assert_eq!(state["initialVersions"]["removed-package"], "3.4.5");
    }

    #[test]
    fn ambiguous_historical_bare_baseline_reports_e245_and_writes_nothing() {
        let temp = workspace(true, ".changeset");
        let root = temp.path();
        let text = json!({"mode":"pre","tag":"beta","initialVersions":{"shared":"1.2.3"},"changesets":[]}).to_string();
        write(root, ".changeset/pre.json", &text);
        commit(root);
        run(
            root,
            &[
                "add",
                "--package",
                "pypi/shared:minor",
                "--summary",
                "Ambiguous baseline",
            ],
        );
        let output = callisto(root, &["version", "--no-refresh-lockfiles"]);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("E245"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_versions(root, "1.2.3", "1.0.0", "2.0.0");
        assert_eq!(fs::read_to_string(root.join(".changeset/pre.json")).unwrap(), text);
        assert!(!root.join("packages/python/CHANGELOG.md").exists());
    }

    /// Promoted workspace: `py-lib` exists on PyPI and npm; `pre.json` holds the legacy bare key and a qualified key.
    fn promoted_with_both_baselines(legacy: &str, canonical: &str) -> (tempfile::TempDir, String) {
        let temp = workspace(false, ".changeset");
        let root = temp.path();
        run(root, &["pre", "enter", "beta"]);
        commit(root);
        write(
            root,
            "packages/sibling/package.json",
            r#"{"name":"py-lib","version":"3.0.0"}"#,
        );
        let mut state = pre_json(root, ".changeset");
        state["initialVersions"]["py-lib"] = json!(legacy);
        state["initialVersions"]["pypi/py-lib"] = json!(canonical);
        let text = state.to_string();
        write(root, ".changeset/pre.json", &text);
        run(
            root,
            &[
                "add",
                "--package",
                "pypi/py-lib:minor",
                "--summary",
                "Conflicting baselines",
            ],
        );
        (temp, text)
    }

    #[test]
    fn differing_legacy_and_qualified_baselines_fail_naming_both_and_write_nothing() {
        let (temp, text) = promoted_with_both_baselines("1.2.3", "1.1.0");
        let root = temp.path();
        for args in [
            &["version", "--no-refresh-lockfiles"][..],
            &["status"],
            &["pre", "exit"],
        ] {
            let output = callisto(root, args);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success(), "callisto {args:?} succeeded");
            assert!(stderr.contains("E305"), "callisto {args:?}: {stderr}");
            for needle in ["`py-lib`", "`pypi/py-lib`", "1.2.3", "1.1.0"] {
                assert!(stderr.contains(needle), "callisto {args:?} missing {needle}: {stderr}");
            }
            assert_eq!(fs::read_to_string(root.join(".changeset/pre.json")).unwrap(), text);
            assert_versions(root, "1.2.3", "1.0.0", "2.0.0");
            assert!(!root.join("packages/python/CHANGELOG.md").exists());
        }
    }

    #[test]
    fn equal_legacy_and_qualified_baselines_coalesce_under_the_qualified_key() {
        let (temp, _) = promoted_with_both_baselines("1.2.3", "1.2.3");
        let root = temp.path();
        version(root);
        assert_versions(root, "1.3.0b0", "1.0.0", "2.0.0");
        let state = pre_json(root, ".changeset");
        assert_eq!(state["initialVersions"]["pypi/py-lib"], "1.2.3");
        assert!(state["initialVersions"].get("py-lib").is_none());
    }

    #[test]
    fn an_unpromoted_qualified_baseline_that_differs_from_the_bare_one_fails() {
        let temp = workspace(false, ".changeset");
        let root = temp.path();
        run(root, &["pre", "enter", "beta"]);
        let mut state = pre_json(root, ".changeset");
        state["initialVersions"]["pypi/py-lib"] = json!("1.1.0");
        let text = state.to_string();
        write(root, ".changeset/pre.json", &text);
        run(
            root,
            &["add", "--package", "py-lib:minor", "--summary", "Conflicting baselines"],
        );
        let output = callisto(root, &["version", "--no-refresh-lockfiles"]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "unexpected success");
        assert!(stderr.contains("E305"), "{stderr}");
        assert_eq!(fs::read_to_string(root.join(".changeset/pre.json")).unwrap(), text);
        assert_versions(root, "1.2.3", "1.0.0", "2.0.0");
    }

    #[test]
    fn malformed_and_foreign_grammar_baselines_fail_before_any_version_writes() {
        for (key, value) in [
            ("py-lib", json!("not-a-version")),
            ("py-lib", json!("1.2.3-alpha.beta")),
            ("pypi/py-lib", json!("1.2.3-alpha.beta")),
            ("rust-lib", json!("1!2.3rc1")),
            ("web-lib", json!("1!2.3rc1")),
            ("rust-lib", json!("v1.2.3")),
            ("web-lib", json!(" 1.2.3 ")),
            ("py-lib", json!(42)),
        ] {
            let temp = workspace(false, ".changeset");
            let root = temp.path();
            run(root, &["pre", "enter", "beta"]);
            run(
                root,
                &["add", "--package", "py-lib:minor", "--summary", "Invalid baseline"],
            );
            let mut state = pre_json(root, ".changeset");
            state["initialVersions"][key] = value;
            let text = state.to_string();
            write(root, ".changeset/pre.json", &text);
            let output = callisto(root, &["version", "--no-refresh-lockfiles"]);
            assert!(!output.status.success(), "unexpected success for {text}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("E114"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_versions(root, "1.2.3", "1.0.0", "2.0.0");
            assert_eq!(fs::read_to_string(root.join(".changeset/pre.json")).unwrap(), text);
            assert!(!root.join("packages/python/CHANGELOG.md").exists());
            assert!(fs::read_dir(root.join(".changeset")).unwrap().any(|entry| {
                let entry = entry.unwrap();
                entry.path().extension().is_some_and(|extension| extension == "md") && entry.file_name() != "README.md"
            }));
        }
    }

    #[test]
    fn bom_prefixed_pre_json_keeps_its_format_when_exiting() {
        let temp = workspace(false, ".changeset");
        let root = temp.path();
        run(root, &["pre", "enter", "alpha"]);
        let path = root.join(".changeset/pre.json");
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, format!("\u{FEFF}{text}")).unwrap();
        run(root, &["pre", "exit"]);
        assert!(fs::read_to_string(path).unwrap().starts_with('\u{FEFF}'));
    }

    #[test]
    fn malformed_owned_baselines_prevent_exit_and_resume_writes() {
        for (mode, args) in [("pre", &["pre", "exit"][..]), ("exit", &["pre", "enter", "beta"][..])] {
            let temp = workspace(false, ".changeset");
            let root = temp.path();
            let text = json!({"mode":mode,"tag":"alpha","initialVersions":{"py-lib":"1.2.3-alpha.beta"},"changesets":["recorded"]}).to_string();
            write(root, ".changeset/pre.json", &text);
            let output = callisto(root, args);
            assert!(!output.status.success());
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("E114"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(fs::read_to_string(root.join(".changeset/pre.json")).unwrap(), text);
            assert_versions(root, "1.2.3", "1.0.0", "2.0.0");
        }
    }

    #[test]
    fn resuming_a_python_cycle_preserves_its_native_baseline_and_recorded_changesets() {
        let temp = workspace(false, ".changeset");
        let root = temp.path();
        run(root, &["pre", "enter", "alpha"]);
        run(
            root,
            &["add", "--package", "py-lib:minor", "--summary", "Resumed Python cycle"],
        );
        version(root);
        assert_versions(root, "1.3.0a0", "1.0.0", "2.0.0");
        run(root, &["pre", "exit"]);
        let exited = pre_json(root, ".changeset");
        run(root, &["pre", "enter", "beta"]);
        let resumed = pre_json(root, ".changeset");
        assert_eq!(resumed["initialVersions"], exited["initialVersions"]);
        assert_eq!(resumed["changesets"], exited["changesets"]);
        assert_eq!(resumed["mode"], "pre");
        assert_eq!(resumed["tag"], "beta");
        version(root);
        version(root);
        assert_versions(root, "1.3.0b1", "1.0.0", "2.0.0");
        let changelog = fs::read_to_string(root.join("packages/python/CHANGELOG.md")).unwrap();
        assert_eq!(changelog.matches("Resumed Python cycle").count(), 1);
    }
}
