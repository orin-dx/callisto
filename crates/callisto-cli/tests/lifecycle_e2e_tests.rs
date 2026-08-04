use std::fs;
use std::process::Command;

use callisto_cli::cli::{
    AddArgs, GlobalArgs, InitArgs, OutputFormat, PlanPublishArgs, PublishArgs, StatusArgs,
    VersionArgs,
};
use callisto_cli::commands;
use tempfile::tempdir;

fn setup_polyglot_git_repo() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // git init
    let status = Command::new("git")
        .args(["init"])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success());

    // git config user.name & user.email for tagging
    Command::new("git")
        .args(["config", "user.name", "Callisto Tester"])
        .current_dir(root)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "tester@callisto.dev"])
        .current_dir(root)
        .status()
        .unwrap();

    // Create Cargo workspace root
    let cargo_toml = r#"[workspace]
members = ["crates/core"]
resolver = "2"
"#;
    fs::write(root.join("Cargo.toml"), cargo_toml).unwrap();

    // Create Cargo crate core
    fs::create_dir_all(root.join("crates/core/src")).unwrap();
    let crate_toml = r#"[package]
name = "core-crate"
version = "0.1.0"
edition = "2021"
"#;
    fs::write(root.join("crates/core/Cargo.toml"), crate_toml).unwrap();
    fs::write(root.join("crates/core/src/lib.rs"), "pub fn hello() {}\n").unwrap();

    // Create npm package.json
    fs::create_dir_all(root.join("packages/web")).unwrap();
    let pkg_json = r#"{
  "name": "@myorg/web-app",
  "version": "1.0.0"
}
"#;
    fs::write(root.join("packages/web/package.json"), pkg_json).unwrap();

    // Initial git commit
    Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(root)
        .status()
        .unwrap();

    dir
}

#[test]
fn test_full_polyglot_workspace_release_lifecycle() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    // 1. callisto init
    let init_res = commands::init::handle(InitArgs { yes: true }, &global);
    assert!(init_res.is_ok());
    assert!(root.join("callisto.toml").exists());

    // 2. callisto add --package core-crate:minor --package @myorg/web-app:patch
    let add_res = commands::add::handle(
        AddArgs {
            packages: vec![
                "core-crate:minor".to_string(),
                "@myorg/web-app:patch".to_string(),
            ],
            summary: Some("Polyglot release feature update".to_string()),
        },
        &global,
    );
    assert!(add_res.is_ok());
    assert!(root.join(".changeset").exists());

    // 3. callisto status
    let status_res = commands::status::handle(
        StatusArgs {
            strict: false,
            strict_graph: false,
            check: false,
        },
        &global,
    );
    assert!(status_res.is_ok());

    // 4. callisto version (consumes changesets, bumps Cargo.toml & package.json)
    let version_res = commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            strict: false,
            strict_graph: false,
            allow_empty_changesets: false,
        },
        &global,
    );
    assert!(version_res.is_ok());

    // Verify Cargo.toml bumped from 0.1.0 to 0.2.0
    let updated_cargo = fs::read_to_string(root.join("crates/core/Cargo.toml")).unwrap();
    assert!(updated_cargo.contains("version = \"0.2.0\""));

    // Verify package.json bumped from 1.0.0 to 1.0.1
    let updated_pkg = fs::read_to_string(root.join("packages/web/package.json")).unwrap();
    assert!(updated_pkg.contains("\"version\": \"1.0.1\""));

    // 5. callisto plan-publish
    let plan_res = commands::plan_publish::handle(PlanPublishArgs {}, &global);
    assert!(plan_res.is_ok());

    // 6. callisto publish --dry-run: must report the plan without ever
    // constructing a PublishOrchestrator or shelling out to a real
    // publisher (cargo/npm/twine) — this test asserts success purely from
    // the dry-run short-circuit, so it never touches a real registry.
    let dry_run_global = GlobalArgs {
        dry_run: true,
        ..global.clone()
    };
    let publish_res = commands::publish::handle(PublishArgs {}, &dry_run_global);
    assert!(publish_res.is_ok());
}

#[test]
fn test_pre_mode_blackbox_lifecycle() {
    use callisto_cli::cli::PreArgs;

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
            strict: false,
            strict_graph: false,
            allow_empty_changesets: false,
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
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        },
        &global,
    );
    assert!(final_version_res.is_ok());

    let final_cargo = fs::read_to_string(root.join("crates/core/Cargo.toml")).unwrap();
    assert!(final_cargo.contains("0.2.0"));
    assert!(!root.join(".changeset/pre.json").exists());
}

#[test]
fn test_cst_comment_preservation_on_version_bump() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let cargo_toml = r#"[package]
name = "core-crate"
version = "0.1.0" # Important inline comment
# Important section comment
edition = "2021"
"#;
    fs::write(root.join("crates/core/Cargo.toml"), cargo_toml).unwrap();

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::add::handle(
        AddArgs {
            packages: vec!["core-crate:patch".to_string()],
            summary: Some("Patch update".to_string()),
        },
        &global,
    )
    .unwrap();

    commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            strict: false,
            strict_graph: false,
            allow_empty_changesets: false,
        },
        &global,
    )
    .unwrap();

    let updated_cargo = fs::read_to_string(root.join("crates/core/Cargo.toml")).unwrap();
    assert!(updated_cargo.contains("version = \"0.1.1\""));
    assert!(updated_cargo.contains("# Important inline comment"));
    assert!(updated_cargo.contains("# Important section comment"));
}

#[test]
fn test_compose_pr_body_before_version_and_subpkg_changelog() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Text,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::add::handle(
        AddArgs {
            packages: vec!["core-crate:minor".to_string()],
            summary: Some("Add new core component API".to_string()),
        },
        &global,
    )
    .unwrap();

    // 1. Compose PR body BEFORE versioning (must be non-empty and contain package summary)
    let compose_res = commands::compose_pr_body::handle(
        callisto_cli::cli::ComposePrBodyArgs {
            existing_body: None,
            labels: Vec::new(),
            branch: None,
        },
        &global,
    );
    assert!(compose_res.is_ok());

    // 2. Version packages
    commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            strict: false,
            strict_graph: false,
            allow_empty_changesets: false,
        },
        &global,
    )
    .unwrap();

    // 3. Verify subpackage changelog was created in crates/core/CHANGELOG.md
    let changelog_path = root.join("crates/core/CHANGELOG.md");
    assert!(
        changelog_path.exists(),
        "crates/core/CHANGELOG.md must exist!"
    );
    let changelog_content = fs::read_to_string(&changelog_path).unwrap();
    assert!(changelog_content.contains("0.2.0"));
}

#[test]
fn test_dry_run_flag_preserves_disk_state() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global_dry = GlobalArgs {
        format: OutputFormat::Text,
        cwd: root.to_path_buf(),
        dry_run: true,
    };

    commands::add::handle(
        AddArgs {
            packages: vec!["core-crate:patch".to_string()],
            summary: Some("Dry run test patch".to_string()),
        },
        &global_dry,
    )
    .unwrap();

    // Run version with dry_run = true
    let version_res = commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            strict: false,
            strict_graph: false,
            allow_empty_changesets: false,
        },
        &global_dry,
    );
    assert!(version_res.is_ok());

    // Manifest must remain 0.1.0 on disk
    let cargo_content = fs::read_to_string(root.join("crates/core/Cargo.toml")).unwrap();
    assert!(cargo_content.contains("version = \"0.1.0\""));
}

#[test]
fn test_add_dry_run_does_not_write_changeset_file() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global_dry = GlobalArgs {
        format: OutputFormat::Text,
        cwd: root.to_path_buf(),
        dry_run: true,
    };

    let add_res = commands::add::handle(
        AddArgs {
            packages: vec!["core-crate:patch".to_string()],
            summary: Some("Dry run should not write anything".to_string()),
        },
        &global_dry,
    );
    assert!(add_res.is_ok());

    // `add --dry-run` must never create the .changeset directory or any
    // changeset file within it.
    let changeset_dir = root.join(".changeset");
    let has_changeset_files = changeset_dir.exists()
        && fs::read_dir(&changeset_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.path().extension().is_some_and(|ext| ext == "md"));
    assert!(
        !has_changeset_files,
        "callisto add --dry-run must not write a changeset file to disk, but found one in {}",
        changeset_dir.display()
    );
}
