use callisto_cli::cli::{GlobalArgs, OutputFormat, StatusArgs};
use callisto_cli::commands;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_e2e_workspace_init_add_and_status() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Initialize git repository
    callisto_fixtures::git::init_repo(root);

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
    for args in [
        ["remote", "add", "origin", "https://github.com/example/my-app.git"].as_slice(),
        ["add", "."].as_slice(),
        ["commit", "-q", "-m", "init"].as_slice(),
    ] {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }
    let init_res = commands::init::handle(
        callisto_cli::cli::InitArgs {
            yes: true,
            versioning: Some(callisto_cli::cli::InitVersioning::Independent),
            ..Default::default()
        },
        &global,
    );
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

    callisto_fixtures::git::init_repo(root);

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

    callisto_fixtures::scaffold_callisto(&global.cwd);

    // Write a changeset manually using the ecosystem-qualified entry name.
    // This happens when a user runs `callisto add --package cargo/my-app:patch`.
    let changeset_dir = root.join(".changeset");
    fs::write(
        changeset_dir.join("fix-ecosystem-name.md"),
        "---\n\"cargo/my-app\": patch\n---\n\nFix using ecosystem-qualified name.\n",
    )
    .unwrap();

    // status --check must still succeed (no error-level diagnostics) --
    // Pending state never affects the exit code, so the
    // ecosystem-qualified-match regression this test guards is now asserted
    // via the report itself (`pending`/`pending_severity`), not the exit code.
    let code = commands::status::handle(
        StatusArgs {
            strict: false,
            check: true,
        },
        &global,
    )
    .unwrap();
    assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));

    let runner = callisto_cli::runner::CliCommandRunner;
    let ws = callisto_cli::workspace::load_workspace(&global, &runner).unwrap();
    let inference = callisto_cli::workspace::select_inference();
    let report =
        callisto_graph::commands::status(&ws, &inference, &callisto_graph::commands::StatusOptions::default()).unwrap();

    assert_eq!(
        report.pending, 1,
        "status must detect a pending changeset whose entry uses an ecosystem-qualified \
         name (cargo/my-app) for a package registered as my-app; got: {:?}",
        report.packages
    );
}

/// `callisto status --check` must return exit code 0 regardless of pending
/// state, as long as there are no error-level diagnostics
/// (`--check` is a conventional errors-only gate).
#[test]
fn test_status_check_exit_codes() {
    use std::process::ExitCode;

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    callisto_fixtures::git::init_repo(root);

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
    callisto_fixtures::scaffold_callisto(&global.cwd);

    let check_args = StatusArgs {
        strict: false,
        check: true,
    };

    // --- Clean workspace: nothing pending, no errors -> exit code 0 ---
    let clean_code = commands::status::handle(check_args.clone(), &global).unwrap();
    assert_eq!(
        format!("{clean_code:?}"),
        format!("{:?}", ExitCode::SUCCESS),
        "clean workspace with --check must return exit code 0"
    );

    // --- Workspace with a pending changeset (no errors) -> still exit code 0 ---
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
        format!("{:?}", ExitCode::SUCCESS),
        "workspace with a well-formed pending changeset and --check must still return \
         exit code 0 -- pending state never gates the exit code"
    );
}

/// `callisto status` must return exit code 0 on a clean workspace without
/// the `--check` flag.
#[test]
fn test_status_default_exit_code_clean_workspace() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    callisto_fixtures::git::init_repo(root);

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

    callisto_fixtures::scaffold_callisto(&global.cwd);

    let code = commands::status::handle(
        StatusArgs {
            strict: false,
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
