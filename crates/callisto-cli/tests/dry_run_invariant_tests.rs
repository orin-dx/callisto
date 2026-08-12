//! The uniform `--dry-run` invariant, applied to every write-capable command.
//!
//! Each test below drives one command with `dry_run: true` inside
//! [`assert_no_disk_mutation`], which compares the entire workspace tree
//! (including `.git`, so staging and tagging are covered) before and after.
//! This is deliberately not per-command bespoke: the previous ad hoc
//! assertions each checked one file the author happened to think of, which is
//! exactly how `pre enter`, `pre exit`, and `init` shipped without ever
//! consulting the dry-run flag at all.

use std::fs;
use std::process::Command;

use callisto_cli::cli::{
    AddArgs, GlobalArgs, InitArgs, OutputFormat, PreArgs, PublishArgs, SnapshotArgs, TagArgs,
    VersionArgs,
};
use callisto_cli::commands;
use callisto_fixtures::dry_run::assert_no_disk_mutation;
use tempfile::tempdir;

fn git(root: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// A committed Cargo + npm workspace, matching the lifecycle suite's fixture.
fn setup_repo() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let root = dir.path();

    git(root, &["init"]);
    git(root, &["config", "user.name", "Callisto Tester"]);
    git(root, &["config", "user.email", "tester@callisto.dev"]);

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
    fs::write(root.join("crates/core/src/lib.rs"), "pub fn hello() {}\n").unwrap();

    fs::create_dir_all(root.join("packages/web")).unwrap();
    fs::write(
        root.join("packages/web/package.json"),
        "{\n  \"name\": \"@myorg/web-app\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .unwrap();

    git(root, &["add", "."]);
    git(root, &["commit", "-m", "Initial commit"]);

    dir
}

fn global(root: &std::path::Path, dry_run: bool) -> GlobalArgs {
    GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run,
    }
}

/// Brings the workspace to a state with `callisto.toml` and one pending
/// changeset, so `version`/`snapshot` have real work to preview.
fn seed_initialized_workspace(root: &std::path::Path) {
    let real = global(root, false);
    commands::init::handle(InitArgs { yes: true }, &real).unwrap();
    commands::add::handle(
        AddArgs {
            packages: vec!["core-crate:minor".to_string()],
            summary: Some("Seeded changeset".to_string()),
        },
        &real,
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "Seed changesets"]);
}

/// Builds a publish plan from the real workspace, so the package identities in
/// it are exactly the ones the tag index knows about.
fn publish_plan_json(root: &std::path::Path) -> String {
    let runner = callisto_cli::CliCommandRunner;
    let ws = callisto_cli::workspace::load_workspace(&global(root, false), &runner).unwrap();
    let plan = callisto_graph::commands::plan_publish(
        &ws,
        &callisto_graph::commands::PublishOptions::default(),
    )
    .unwrap();
    serde_json::to_string(&plan).unwrap()
}

#[test]
fn add_dry_run_writes_nothing() {
    let dir = setup_repo();
    let root = dir.path();
    seed_initialized_workspace(root);

    assert_no_disk_mutation(root, || {
        commands::add::handle(
            AddArgs {
                packages: vec!["core-crate:patch".to_string()],
                summary: Some("Should not be written".to_string()),
            },
            &global(root, true),
        )
        .unwrap();
    });
}

#[test]
fn version_dry_run_writes_nothing() {
    let dir = setup_repo();
    let root = dir.path();
    seed_initialized_workspace(root);

    assert_no_disk_mutation(root, || {
        commands::version::handle(
            VersionArgs {
                refresh_lockfiles: false,
                strict: false,
                strict_graph: false,
                allow_empty_changesets: false,
            },
            &global(root, true),
        )
        .unwrap();
    });
}

#[test]
fn snapshot_dry_run_writes_nothing() {
    let dir = setup_repo();
    let root = dir.path();
    seed_initialized_workspace(root);

    assert_no_disk_mutation(root, || {
        commands::snapshot::handle(
            SnapshotArgs {
                tag: "canary".to_string(),
                strict: false,
            },
            &global(root, true),
        )
        .unwrap();
    });
}

/// Regression: `pre enter` wrote `.changeset/pre.json` unconditionally,
/// never consulting `global.dry_run`.
#[test]
fn pre_enter_dry_run_writes_nothing() {
    let dir = setup_repo();
    let root = dir.path();
    seed_initialized_workspace(root);

    assert_no_disk_mutation(root, || {
        commands::pre::handle(
            PreArgs::Enter {
                tag: "beta".to_string(),
            },
            &global(root, true),
        )
        .unwrap();
    });
}

/// Regression: `pre exit` rewrote an existing `.changeset/pre.json`
/// unconditionally, never consulting `global.dry_run`.
#[test]
fn pre_exit_dry_run_writes_nothing() {
    let dir = setup_repo();
    let root = dir.path();
    seed_initialized_workspace(root);

    // Establish real pre-mode state first, so `pre exit` has a file to mutate.
    commands::pre::handle(
        PreArgs::Enter {
            tag: "beta".to_string(),
        },
        &global(root, false),
    )
    .unwrap();

    assert_no_disk_mutation(root, || {
        commands::pre::handle(PreArgs::Exit, &global(root, true)).unwrap();
    });
}

/// Regression: `init` scaffolded `callisto.toml`, `.changeset/`, and
/// `.changeset/README.md` unconditionally on a first run.
#[test]
fn init_dry_run_writes_nothing_on_first_run() {
    let dir = setup_repo();
    let root = dir.path();

    assert_no_disk_mutation(root, || {
        commands::init::handle(InitArgs { yes: false }, &global(root, true)).unwrap();
    });
}

/// `--yes` and `--dry-run` are independent gates. `--yes` only answers the
/// interactive confirm and the reconcile-apply question; it must never
/// override `--dry-run`. This combination was the specific hole: `--yes`
/// took the apply branch while `--dry-run` was ignored entirely.
#[test]
fn init_dry_run_writes_nothing_even_with_yes() {
    let dir = setup_repo();
    let root = dir.path();

    assert_no_disk_mutation(root, || {
        commands::init::handle(InitArgs { yes: true }, &global(root, true)).unwrap();
    });
}

/// The reconcile path: an existing `callisto.toml` recording fewer ecosystems
/// than the workspace now contains. With `--yes` this normally rewrites the
/// `[init]` table; under `--dry-run` it must only report the diff.
#[test]
fn init_dry_run_writes_nothing_on_reconcile_with_yes() {
    let dir = setup_repo();
    let root = dir.path();

    // Record only `cargo`, leaving `npm` as drift to reconcile.
    fs::write(
        root.join("callisto.toml"),
        "# callisto configuration\n\n[init]\necosystems = [\"cargo\"]\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(root.join(".changeset/README.md"), "# Changesets\n").unwrap();

    assert_no_disk_mutation(root, || {
        commands::init::handle(InitArgs { yes: true }, &global(root, true)).unwrap();
    });
}

#[test]
fn tag_dry_run_writes_nothing() {
    let dir = setup_repo();
    let root = dir.path();
    seed_initialized_workspace(root);

    let plan = publish_plan_json(root);

    assert_no_disk_mutation(root, || {
        commands::tag::handle(
            TagArgs {
                plan,
                floating_major: true,
                strict: false,
            },
            &global(root, true),
        )
        .unwrap();
    });
}

#[test]
fn publish_dry_run_writes_nothing() {
    let dir = setup_repo();
    let root = dir.path();
    seed_initialized_workspace(root);

    assert_no_disk_mutation(root, || {
        commands::publish::handle(PublishArgs { only: vec![] }, &global(root, true)).unwrap();
    });
}

/// `snapshot --dry-run --format text` must prefix output with `[DRY-RUN]` so
/// users can distinguish a preview from a real run.
#[test]
fn snapshot_dry_run_text_output_has_dry_run_marker() {
    let dir = setup_repo();
    let root = dir.path();
    seed_initialized_workspace(root);

    let out = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args([
            "--format",
            "text",
            "--dry-run",
            "--cwd",
            root.to_str().unwrap(),
            "snapshot",
            "--tag",
            "canary",
        ])
        .output()
        .expect("callisto binary should be invocable");

    assert!(
        out.status.success(),
        "snapshot --dry-run should exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let text = String::from_utf8(out.stdout).expect("stdout is UTF-8");
    assert!(
        text.contains("[DRY-RUN]"),
        "snapshot --dry-run text output should contain [DRY-RUN], got: {text:?}"
    );
}

/// `tag --dry-run --format text` must not claim tags were created when none
/// were. The output should indicate dry-run mode rather than asserting "Created
/// Tags:".
#[test]
fn tag_dry_run_text_output_does_not_say_created_tags() {
    let dir = setup_repo();
    let root = dir.path();
    seed_initialized_workspace(root);

    let plan_json = publish_plan_json(root);
    let plan_file = root.join("plan.json");
    fs::write(&plan_file, &plan_json).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args([
            "--format",
            "text",
            "--dry-run",
            "--cwd",
            root.to_str().unwrap(),
            "tag",
            "--plan",
            plan_file.to_str().unwrap(),
        ])
        .output()
        .expect("callisto binary should be invocable");

    assert!(
        out.status.success(),
        "tag --dry-run should exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let text = String::from_utf8(out.stdout).expect("stdout is UTF-8");
    assert!(
        !text.contains("Created Tags:"),
        "tag --dry-run text output must not say 'Created Tags:', got: {text:?}"
    );
    assert!(
        text.contains("Would create tags:"),
        "tag --dry-run text output should say 'Would create tags:', got: {text:?}"
    );
}

/// `pre exit` reads `.changeset/pre.json` before checking the dry-run permit.
/// When the file does not exist the function must return `CliError::Io`, not
/// panic or produce a misleading error variant.
#[test]
fn pre_exit_without_pre_json_returns_io_error() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // A minimal workspace root marker satisfies `find_workspace_root`.
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = []\nresolver = \"2\"\n",
    )
    .unwrap();

    let g = GlobalArgs {
        format: OutputFormat::Text,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    let result = commands::pre::handle(PreArgs::Exit, &g);
    assert!(
        matches!(result, Err(callisto_cli::error::CliError::Io { .. })),
        "expected CliError::Io when .changeset/pre.json is absent, got: {:?}",
        result
    );
}

/// The `preview()` function emits a JSON envelope with seven fields:
/// `schemaVersion`, `command`, `dryRun`, `mode`, `tag`, `path`, `content`.
/// This test runs the real binary for both `pre enter` and `pre exit` under
/// `--dry-run --format json` and asserts every field is present and typed
/// correctly.
#[test]
fn pre_dry_run_json_output_fields_are_correct() {
    let dir = setup_repo();
    let root = dir.path();
    seed_initialized_workspace(root);

    // --- pre enter --dry-run --format json ---
    let enter_out = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args([
            "--format",
            "json",
            "--dry-run",
            "--cwd",
            root.to_str().unwrap(),
            "pre",
            "enter",
            "beta",
        ])
        .output()
        .expect("callisto binary should be invocable");

    assert!(
        enter_out.status.success(),
        "pre enter --dry-run should exit 0; stderr: {}",
        String::from_utf8_lossy(&enter_out.stderr)
    );

    let enter_text = String::from_utf8(enter_out.stdout).expect("stdout is UTF-8");
    let enter_json: serde_json::Value =
        serde_json::from_str(&enter_text).expect("dry-run output must be valid JSON");

    assert_eq!(enter_json["schemaVersion"], 1, "schemaVersion should be 1");
    assert_eq!(enter_json["command"], "pre", "command should be \"pre\"");
    assert_eq!(enter_json["dryRun"], true, "dryRun should be true");
    assert_eq!(
        enter_json["mode"], "pre",
        "mode should be \"pre\" for enter"
    );
    assert_eq!(
        enter_json["tag"], "beta",
        "tag should echo the supplied tag"
    );
    assert_eq!(
        enter_json["path"], ".changeset/pre.json",
        "path should be the relative pre.json path"
    );
    assert!(
        enter_json["content"].is_string(),
        "content should be a JSON string"
    );

    // Establish real pre mode so `pre exit` has a file to read.
    commands::pre::handle(
        PreArgs::Enter {
            tag: "beta".to_string(),
        },
        &global(root, false),
    )
    .expect("real pre enter should succeed");

    // --- pre exit --dry-run --format json ---
    let exit_out = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args([
            "--format",
            "json",
            "--dry-run",
            "--cwd",
            root.to_str().unwrap(),
            "pre",
            "exit",
        ])
        .output()
        .expect("callisto binary should be invocable");

    assert!(
        exit_out.status.success(),
        "pre exit --dry-run should exit 0; stderr: {}",
        String::from_utf8_lossy(&exit_out.stderr)
    );

    let exit_text = String::from_utf8(exit_out.stdout).expect("stdout is UTF-8");
    let exit_json: serde_json::Value =
        serde_json::from_str(&exit_text).expect("dry-run output must be valid JSON");

    assert_eq!(exit_json["schemaVersion"], 1, "schemaVersion should be 1");
    assert_eq!(exit_json["command"], "pre", "command should be \"pre\"");
    assert_eq!(exit_json["dryRun"], true, "dryRun should be true");
    assert_eq!(
        exit_json["mode"], "exit",
        "mode should be \"exit\" for exit"
    );
    assert_eq!(
        exit_json["tag"], "beta",
        "tag should be the tag from pre.json"
    );
    assert_eq!(
        exit_json["path"], ".changeset/pre.json",
        "path should be the relative pre.json path"
    );
    assert!(
        exit_json["content"].is_string(),
        "content should be a JSON string"
    );
}
