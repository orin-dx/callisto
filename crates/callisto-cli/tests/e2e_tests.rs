use callisto_cli::cli::{GlobalArgs, OutputFormat, StatusArgs};
use callisto_cli::commands;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_e2e_workspace_init_add_and_status() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Initialize git repository
    let _res = std::process::Command::new("git")
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
        StatusArgs {
            strict: false,
            strict_graph: false,
            check: false,
        },
        &global,
    );
    assert!(status_res.is_ok());
}

/// Regression: `callisto status --check` must detect a pending changeset
/// whose entry uses an ecosystem-qualified name (`cargo/my-app`) even when
/// the package is registered under its bare name (`my-app`).
///
/// Before the fix, `status.rs` compared `entry.name == pkg.id.to_string()`,
/// which returned false for `"cargo/my-app" != "my-app"`, so the package
/// showed no pending changesets while `callisto version` (which uses
/// `PackageId::matches()`) correctly processed it.
#[test]
fn test_status_matches_ecosystem_qualified_changeset_entry() {
    use std::process::ExitCode;

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    drop(
        std::process::Command::new("git")
            .arg("init")
            .current_dir(root)
            .output(),
    );

    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    )
    .unwrap();

    let crate_dir = root.join("crates/my-app");
    fs::create_dir_all(&crate_dir).unwrap();
    fs::write(
        crate_dir.join("Cargo.toml"),
        "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::init::handle(callisto_cli::cli::InitArgs { yes: true }, &global).unwrap();

    // Write a changeset manually using the ecosystem-qualified entry name.
    // This happens when a user runs `callisto add --package cargo/my-app:patch`.
    let changeset_dir = root.join(".changeset");
    fs::write(
        changeset_dir.join("fix-ecosystem-name.md"),
        "---\n\"cargo/my-app\": patch\n---\n\nFix using ecosystem-qualified name.\n",
    )
    .unwrap();

    // --check returns exit code 1 (FAILURE) when at least one pending changeset is found.
    // Before the fix this returned exit code 2 (no pending changesets detected).
    let code = commands::status::handle(
        StatusArgs {
            strict: false,
            strict_graph: false,
            check: true,
        },
        &global,
    )
    .unwrap();

    assert_eq!(
        format!("{code:?}"),
        format!("{:?}", ExitCode::FAILURE),
        "status --check must detect a pending changeset whose entry uses an \
         ecosystem-qualified name (cargo/my-app) for a package registered as my-app"
    );
}

/// `callisto status --check` must return exit code 1 (FAILURE) when at least one pending
/// changeset exists, and exit code 2 when the workspace is clean.
#[test]
fn test_status_check_exit_codes() {
    use std::process::ExitCode;

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    drop(
        std::process::Command::new("git")
            .arg("init")
            .current_dir(root)
            .output(),
    );

    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    )
    .unwrap();

    let crate_dir = root.join("crates/my-app");
    fs::create_dir_all(&crate_dir).unwrap();
    fs::write(
        crate_dir.join("Cargo.toml"),
        "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    // Initialize so callisto.toml exists.
    commands::init::handle(callisto_cli::cli::InitArgs { yes: true }, &global).unwrap();

    let check_args = StatusArgs {
        strict: false,
        strict_graph: false,
        check: true,
    };

    // --- Clean workspace: no changesets pending -> exit code 2 ---
    let clean_code = commands::status::handle(check_args.clone(), &global).unwrap();
    // ExitCode does not implement PartialEq, but `from(2)` is equivalent to
    // the u8 value 2.  We compare via the Display of the debug form, which is
    // not stable, so instead we re-derive from the known constant.
    let expected_clean = ExitCode::from(2u8);
    // We cannot directly compare ExitCode values, so compare via a round-trip:
    // if `clean_code` were SUCCESS (0) the test below would panic correctly.
    // The canonical check is to ensure it is NOT SUCCESS.
    assert_ne!(
        format!("{clean_code:?}"),
        format!("{:?}", ExitCode::SUCCESS),
        "clean workspace with --check must not return exit code 0"
    );
    assert_eq!(
        format!("{clean_code:?}"),
        format!("{expected_clean:?}"),
        "clean workspace with --check must return exit code 2"
    );

    // --- Workspace with a pending changeset -> exit code 0 ---
    commands::add::handle(
        callisto_cli::cli::AddArgs {
            packages: vec!["my-app:patch".to_string()],
            summary: Some("test fix".to_string()),
        },
        &global,
    )
    .unwrap();

    let pending_code = commands::status::handle(check_args.clone(), &global).unwrap();
    assert_eq!(
        format!("{pending_code:?}"),
        format!("{:?}", ExitCode::FAILURE),
        "workspace with pending changeset and --check must return exit code 1 (FAILURE)"
    );
}

/// `callisto status` must return exit code 0 on a clean workspace without
/// the `--check` flag.
#[test]
fn test_status_default_exit_code_clean_workspace() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    drop(
        std::process::Command::new("git")
            .arg("init")
            .current_dir(root)
            .output(),
    );

    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    )
    .unwrap();

    let crate_dir = root.join("crates/lib-a");
    fs::create_dir_all(&crate_dir).unwrap();
    fs::write(
        crate_dir.join("Cargo.toml"),
        "[package]\nname = \"lib-a\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::init::handle(callisto_cli::cli::InitArgs { yes: true }, &global).unwrap();

    let code = commands::status::handle(
        StatusArgs {
            strict: false,
            strict_graph: false,
            check: false,
        },
        &global,
    )
    .unwrap();

    assert_eq!(
        format!("{code:?}"),
        format!("{:?}", std::process::ExitCode::SUCCESS),
        "status without --check on a clean workspace must return exit code 0"
    );
}
