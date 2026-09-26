//! After `version`/`snapshot`, each ecosystem's locked install must still succeed against a real subprocess.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use callisto_cli::cli::{AddArgs, GlobalArgs, OutputFormat, SnapshotArgs, VersionArgs};
use callisto_cli::commands;

/// Finds `program` on `PATH`, the way a shell would; never panics, so a tool-specific assertion can skip cleanly.
fn find_on_path(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}

/// A mixed Cargo + npm workspace with real lockfiles and cross-workspace dependency edges, committed to git.
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

    // Lockfiles are generated before any callisto command runs, so the later check is a real regression test.
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

/// `cargo metadata --locked --offline` always; `npm ci` too, when npm is on PATH.
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

/// A pnpm workspace, plus a real uv one when `include_python`; false for snapshot -- see ROAD-TO-V1.md 2b.
fn setup_pnpm_and_uv_workspace(root: &Path, include_python: bool) {
    callisto_fixtures::git::init_repo(root);

    fs::write(
        root.join("package.json"),
        r#"{"name":"pnpm-root","private":true,"version":"0.0.0"}"#,
    )
    .unwrap();
    fs::write(root.join("pnpm-workspace.yaml"), "packages:\n  - \"packages/*\"\n").unwrap();
    fs::create_dir_all(root.join("packages/lib")).unwrap();
    fs::write(
        root.join("packages/lib/package.json"),
        r#"{"name":"@ws/lib","version":"1.0.0"}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("packages/app")).unwrap();
    fs::write(
        root.join("packages/app/package.json"),
        r#"{"name":"@ws/app","version":"1.0.0","dependencies":{"@ws/lib":"workspace:^1.0.0"}}"#,
    )
    .unwrap();

    if include_python {
        fs::write(
            root.join("pyproject.toml"),
            "[tool.uv.workspace]\nmembers = [\"py/core\", \"py/app\"]\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("py/core")).unwrap();
        fs::write(
            root.join("py/core/pyproject.toml"),
            "[project]\nname = \"core-py\"\nversion = \"1.0.0\"\nrequires-python = \">=3.9\"\ndependencies = []\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("py/app")).unwrap();
        fs::write(
            root.join("py/app/pyproject.toml"),
            "[project]\nname = \"app-py\"\nversion = \"1.0.0\"\nrequires-python = \">=3.9\"\n\
             dependencies = [\"core-py>=1.0.0,<2.0.0\"]\n\n\
             [tool.uv.sources]\ncore-py = { workspace = true }\n",
        )
        .unwrap();
    }

    if find_on_path("pnpm").is_some() {
        assert!(Command::new("pnpm")
            .args(["install", "--lockfile-only"])
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }
    if include_python && find_on_path("uv").is_some() {
        assert!(Command::new("uv")
            .args(["lock"])
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

/// `pnpm install --frozen-lockfile` always; `uv lock --check` too when `include_python`; each tool-gated.
fn assert_pnpm_and_uv_resolution(root: &Path, include_python: bool) {
    if find_on_path("pnpm").is_some() {
        let pnpm_out = Command::new("pnpm")
            .args(["install", "--frozen-lockfile"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            pnpm_out.status.success(),
            "pnpm install --frozen-lockfile must succeed after the workspace's manifests are bumped; stderr: {}",
            String::from_utf8_lossy(&pnpm_out.stderr)
        );
    }

    if include_python && find_on_path("uv").is_some() {
        let uv_out = Command::new("uv")
            .args(["lock", "--check"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            uv_out.status.success(),
            "uv lock --check must succeed after the workspace's manifests are bumped; stderr: {}",
            String::from_utf8_lossy(&uv_out.stderr)
        );
    }
}

#[test]
fn pnpm_and_uv_workspace_resolves_natively_after_version() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup_pnpm_and_uv_workspace(root, true);

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::add::handle(
        AddArgs {
            packages: vec!["@ws/lib:major".to_string(), "core-py:major".to_string()],
            summary: Some("Bump lib and core-py".to_string()),
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

    assert_pnpm_and_uv_resolution(root, true);
}

/// No Python package here: see `setup_pnpm_and_uv_workspace`'s doc comment for why `snapshot` can't take one.
#[test]
fn pnpm_workspace_resolves_natively_after_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup_pnpm_and_uv_workspace(root, false);

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

    assert_pnpm_and_uv_resolution(root, false);
}
