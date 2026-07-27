use callisto_cli::cli::{GlobalArgs, OutputFormat};
use callisto_cli::commands;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_e2e_workspace_init_add_and_status() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Initialize git repository
    let _ = std::process::Command::new("git")
        .arg("init")
        .current_dir(root)
        .output();

    // Create a Cargo workspace manifest
    let cargo_toml = r#"[workspace]
members = ["crates/*"]
resolver = "2"
"#;
    fs::write(root.join("Cargo.toml"), cargo_toml).unwrap();

    let crate_dir = root.join("crates/my-app");
    fs::create_dir_all(&crate_dir).unwrap();

    let crate_toml = r#"[package]
name = "my-app"
version = "0.1.0"
edition = "2021"
"#;
    fs::write(crate_dir.join("Cargo.toml"), crate_toml).unwrap();

    // 1. Run init
    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };
    let init_res = commands::init::handle(callisto_cli::cli::InitArgs { yes: true }, &global);
    assert!(init_res.is_ok());
    assert!(root.join("callisto.toml").exists());

    // 2. Run add
    let add_res = commands::add::handle(
        callisto_cli::cli::AddArgs {
            packages: vec!["my-app:minor".to_string()],
            summary: Some("Added new feature".to_string()),
        },
        &global,
    );
    assert!(add_res.is_ok());
    let changeset_files: Vec<_> = fs::read_dir(root.join(".changeset"))
        .unwrap()
        .flatten()
        .filter(|e| {
            e.path().extension().and_then(|ext| ext.to_str()) == Some("md")
                && e.path().file_name().and_then(|n| n.to_str()) != Some("README.md")
        })
        .collect();
    assert_eq!(changeset_files.len(), 1);

    // 3. Run status
    let status_res = commands::status::handle(
        callisto_cli::cli::StatusArgs {
            strict: false,
            strict_graph: false,
        },
        &global,
    );
    assert!(status_res.is_ok());
}
