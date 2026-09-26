//! Native-resolution invariant: after `version` and after `snapshot`, each ecosystem's
//! locked install must still succeed against a mixed Cargo + npm workspace with real
//! path/version and cross-workspace dependency edges. See docs/projects/ROAD-TO-V1.md,
//! "The workspace resolves natively after `version` and `snapshot`".

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use callisto_cli::cli::{AddArgs, GlobalArgs, OutputFormat, SnapshotArgs, VersionArgs};
use callisto_cli::commands;

/// Finds `program` on `PATH`, the way a shell would. Never panics, so an npm-specific
/// assertion can skip cleanly when npm isn't installed.
fn find_on_path(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}

/// A mixed Cargo + npm workspace, committed to git with its lockfiles already generated
/// (matching a real checked-in repo): a Cargo path+version dependency edge (`app` on
/// `core`), and an npm workspace (`packages/*`) with a dependent (`@ws/app` on `@ws/lib`
/// via `^1.0.0`).
fn setup_mixed_native_resolution_workspace(root: &Path) {
    callisto_fixtures::git::init_repo(root);

    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/core\", \"crates/app\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/core/src")).unwrap();
    fs::write(
        root.join("crates/core/Cargo.toml"),
        "[package]\nname = \"core\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("crates/core/src/lib.rs"), "pub fn hello() {}\n").unwrap();

    fs::create_dir_all(root.join("crates/app/src")).unwrap();
    fs::write(
        root.join("crates/app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n\
         [dependencies]\ncore = { path = \"../core\", version = \"1.0.0\" }\n",
    )
    .unwrap();
    fs::write(root.join("crates/app/src/lib.rs"), "").unwrap();

    fs::write(
        root.join("package.json"),
        r#"{"name":"root-ws","private":true,"version":"0.0.0","workspaces":["packages/*"]}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("packages/lib")).unwrap();
    fs::write(
        root.join("packages/lib/package.json"),
        r#"{"name":"@ws/lib","version":"1.0.0"}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("packages/app")).unwrap();
    fs::write(
        root.join("packages/app/package.json"),
        r#"{"name":"@ws/app","version":"1.0.0","dependencies":{"@ws/lib":"^1.0.0"}}"#,
    )
    .unwrap();

    // Generate the lockfiles a real repo would have checked in, before any callisto
    // command runs, so a later `--locked`/`ci` check is a genuine regression check
    // instead of a freshly-generated file papering over a stale one.
    assert!(Command::new("cargo")
        .args(["generate-lockfile"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());

    if find_on_path("npm").is_some() {
        assert!(Command::new("npm")
            .args(["install", "--package-lock-only", "--ignore-scripts"])
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }

    assert!(Command::new("git")
        .args(["add", "-A"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-c", "commit.gpgSign=false", "commit", "-q", "-m", "Initial commit"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
}

/// Both ecosystems must resolve natively from their on-disk lockfile: `cargo metadata
/// --locked --offline` always; `npm ci` too, when npm is on PATH (matching the lockfile
/// setup in `setup_mixed_native_resolution_workspace`).
fn assert_native_resolution(root: &Path) {
    let cargo_out = Command::new("cargo")
        .args(["metadata", "--locked", "--offline", "--format-version", "1"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        cargo_out.status.success(),
        "cargo metadata --locked --offline must succeed; stderr: {}",
        String::from_utf8_lossy(&cargo_out.stderr)
    );

    if find_on_path("npm").is_some() {
        let npm_out = Command::new("npm").args(["ci"]).current_dir(root).output().unwrap();
        assert!(
            npm_out.status.success(),
            "npm ci must succeed after the workspace's manifests are bumped; stderr: {}",
            String::from_utf8_lossy(&npm_out.stderr)
        );
    }
}

#[test]
fn workspace_resolves_natively_after_version() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup_mixed_native_resolution_workspace(root);

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::add::handle(
        AddArgs {
            packages: vec!["core:major".to_string(), "@ws/lib:major".to_string()],
            summary: Some("Bump core and lib".to_string()),
        },
        &global,
    )
    .unwrap();

    commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            no_refresh_lockfiles: false,
            strict: false,
            allow_empty_changesets: false,
            emit_decision: None,
        },
        &global,
    )
    .unwrap();

    assert_native_resolution(root);
}

#[test]
fn workspace_resolves_natively_after_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup_mixed_native_resolution_workspace(root);

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::snapshot::handle(
        SnapshotArgs {
            tag: "canary".to_string(),
            strict: false,
        },
        &global,
    )
    .unwrap();

    assert_native_resolution(root);
}
