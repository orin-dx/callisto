use std::fs;
use std::process::Command;

use callisto_cli::cli::{
    AddArgs, GlobalArgs, InitArgs, OutputFormat, PlanPublishArgs, StatusArgs, VersionArgs,
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
}
