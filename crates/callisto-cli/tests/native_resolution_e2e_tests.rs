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
    fs::create_dir_all(root.join("packages/app/src")).unwrap();
    fs::write(root.join("packages/app/src/lib.rs"), "\n").unwrap();
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

fn setup_promoted_dual_npm_snapshot_workspace(root: &Path) {
    callisto_fixtures::git::init_repo(root);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/source\", \"packages/app\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/source/src")).unwrap();
    fs::write(
        root.join("crates/source/Cargo.toml"),
        "[package]\nname = \"source\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("crates/source/src/lib.rs"), "\n").unwrap();
    fs::create_dir_all(root.join("packages/app/src")).unwrap();
    fs::write(root.join("packages/app/src/lib.rs"), "\n").unwrap();
    fs::write(
        root.join("packages/app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        root.join("crates/source/package.json"),
        "{\n  \"name\": \"@native/source\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("packages/source")).unwrap();
    fs::write(
        root.join("packages/source/package.json"),
        "{\n  \"name\": \"source\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("packages/app")).unwrap();
    fs::write(
        root.join("packages/app/package.json"),
        "{\n  \"name\": \"@native/app\",\n  \"version\": \"1.0.0\",\n  \"dependencies\": {\"@native/source\": \"^1.0.0\"}\n}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("packages/app-sibling")).unwrap();
    fs::write(
        root.join("packages/app-sibling/package.json"),
        "{\n  \"name\": \"app\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .unwrap();
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

#[test]
fn snapshot_rewrites_promoted_dual_source_using_its_npm_name() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup_promoted_dual_npm_snapshot_workspace(root);

    let status_output = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--format", "json", "--cwd", root.to_str().unwrap(), "status"])
        .output()
        .unwrap();
    assert!(
        status_output.status.success(),
        "callisto status failed: {}",
        String::from_utf8_lossy(&status_output.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&status_output.stdout).unwrap();
    let status_ids = status["packages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|package| package["package"].as_str())
        .collect::<Vec<_>>();
    assert!(status_ids.contains(&"cargo/app"), "{status}");
    assert!(status_ids.contains(&"npm/app"), "{status}");

    let output = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args([
            "--format",
            "json",
            "--cwd",
            root.to_str().unwrap(),
            "snapshot",
            "--tag",
            "canary",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "callisto snapshot failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let npm_owner: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("crates/source/package.json")).unwrap()).unwrap();
    let snapshot_version = npm_owner["version"].as_str().unwrap();
    assert!(snapshot_version.starts_with("0.0.0-canary-"), "{npm_owner}");
    let dependent: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("packages/app/package.json")).unwrap()).unwrap();
    assert_eq!(dependent["name"], "@native/app");
    assert_eq!(
        dependent["dependencies"]["@native/source"],
        format!("^{snapshot_version}")
    );
    assert!(dependent["dependencies"].get("source").is_none(), "{dependent}");
}

/// A pnpm workspace, plus a real uv one when `include_python`.
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

#[test]
fn pnpm_workspace_version_refreshes_lockfile_for_frozen_install() {
    if find_on_path("pnpm").is_none() {
        eprintln!("native coverage gap: pnpm is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup_pnpm_and_uv_workspace(root, false);
    let lockfile = root.join("pnpm-lock.yaml");
    assert!(lockfile.is_file(), "fixture must start with a pnpm lockfile");

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };
    commands::add::handle(
        AddArgs {
            packages: vec!["@ws/lib:major".to_string()],
            summary: Some("Bump workspace library".to_string()),
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

    let refreshed_lock = fs::read_to_string(&lockfile).unwrap();
    assert!(refreshed_lock.contains("version: link:../lib"), "{refreshed_lock}");
    assert!(
        refreshed_lock.contains("specifier: workspace:^2.0.0"),
        "{refreshed_lock}"
    );
    let frozen = Command::new("pnpm")
        .args(["install", "--frozen-lockfile", "--ignore-scripts", "--offline"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        frozen.status.success(),
        "pnpm install --frozen-lockfile must succeed after version refresh; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&frozen.stdout),
        String::from_utf8_lossy(&frozen.stderr)
    );
}

#[test]
fn pnpm_and_uv_workspace_resolves_natively_after_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup_pnpm_and_uv_workspace(root, true);

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

    assert_pnpm_and_uv_resolution(root, true);
}

fn run_pnpm(root: &Path, args: &[&str]) -> std::process::Output {
    let output = Command::new("pnpm").args(args).current_dir(root).output().unwrap();
    assert!(
        output.status.success(),
        "pnpm {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn setup_pnpm_protocol_workspace(root: &Path) {
    callisto_fixtures::git::init_repo(root);
    fs::write(root.join(".gitignore"), "node_modules/\npacked/\n").unwrap();
    fs::write(
        root.join("package.json"),
        r#"{"name":"protocol-root","version":"1.0.0","private":true}"#,
    )
    .unwrap();
    fs::write(
        root.join("pnpm-workspace.yaml"),
        "packages:\n  - packages/*\nautoInstallPeers: false\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("packages/lib")).unwrap();
    fs::write(
        root.join("packages/lib/package.json"),
        r#"{"name":"@protocol/lib","version":"0.2.3"}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("packages/private-leaf")).unwrap();
    fs::write(
        root.join("packages/private-leaf/package.json"),
        r#"{"name":"private-leaf","version":"0.2.3","private":true}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("packages/app")).unwrap();
    let app = serde_json::json!({
        "name": "protocol-app", "version": "1.0.0", "private": true,
        "dependencies": {
            "empty": "workspace:@protocol/lib@", "star": "workspace:@protocol/lib@*",
            "caret": "workspace:@protocol/lib@^", "tilde": "workspace:@protocol/lib@~",
            "range": "workspace:@protocol/lib@^0.2.0", "pin": "workspace:@protocol/lib@0.2.3",
            "path-lib": "workspace:../lib", "covered": "workspace:@protocol/lib@>=0.2.0"
        },
        "devDependencies": {"dev-pin": "workspace:@protocol/lib@0.2.3"},
        "optionalDependencies": {"optional": "workspace:@protocol/lib@~", "private-leaf": "workspace:*"},
        "peerDependencies": {"@protocol/lib": "workspace:^0.2.0"},
        "peerDependenciesMeta": {"@protocol/lib": {"optional": true}}
    });
    fs::write(
        root.join("packages/app/package.json"),
        serde_json::to_vec_pretty(&app).unwrap(),
    )
    .unwrap();
    run_pnpm(root, &["install", "--lockfile-only", "--ignore-scripts", "--offline"]);
    for args in [
        vec!["add", "-A"],
        vec!["-c", "commit.gpgSign=false", "commit", "-q", "-m", "Initial commit"],
    ] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }
}

fn read_native_package(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn packed_pnpm_manifest(root: &Path) -> serde_json::Value {
    run_pnpm(
        &root.join("packages/app"),
        &["pack", "--pack-destination", "../../packed"],
    );
    let archives: Vec<_> = fs::read_dir(root.join("packed"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(archives.len(), 1);
    let output = Command::new("tar")
        .arg("-xOf")
        .arg(&archives[0])
        .arg("package/package.json")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_pnpm_protocols_and_pack(root: &Path, snapshot: bool) {
    let app_path = root.join("packages/app/package.json");
    let app = read_native_package(&app_path);
    let lib = read_native_package(&root.join("packages/lib/package.json"));
    let version = lib["version"].as_str().unwrap();
    if snapshot {
        assert!(version.starts_with("0.0.0-canary-"), "{version}");
    } else {
        assert_eq!(version, "0.3.0");
    }
    for (alias, suffix) in [("empty", ""), ("star", "*"), ("caret", "^"), ("tilde", "~")] {
        assert_eq!(app["dependencies"][alias], format!("workspace:@protocol/lib@{suffix}"));
    }
    assert_eq!(app["dependencies"]["path-lib"], "workspace:../lib");
    assert_eq!(
        app["dependencies"]["range"],
        format!("workspace:@protocol/lib@^{version}")
    );
    assert_eq!(app["dependencies"]["pin"], format!("workspace:@protocol/lib@{version}"));
    assert_eq!(
        app["devDependencies"]["dev-pin"],
        format!("workspace:@protocol/lib@{version}")
    );
    assert_eq!(app["optionalDependencies"]["optional"], "workspace:@protocol/lib@~");
    assert_eq!(app["optionalDependencies"]["private-leaf"], "workspace:*");
    assert_eq!(
        app["peerDependencies"]["@protocol/lib"],
        format!("workspace:^{version}")
    );
    assert_eq!(app["peerDependenciesMeta"]["@protocol/lib"]["optional"], true);
    let covered = if snapshot {
        format!(">={version}")
    } else {
        ">=0.2.0".to_string()
    };
    assert_eq!(
        app["dependencies"]["covered"],
        format!("workspace:@protocol/lib@{covered}")
    );
    let lock_before = fs::read(root.join("pnpm-lock.yaml")).unwrap();
    run_pnpm(root, &["install", "--frozen-lockfile", "--ignore-scripts", "--offline"]);
    assert_eq!(fs::read(root.join("pnpm-lock.yaml")).unwrap(), lock_before);
    let source_before = fs::read(&app_path).unwrap();
    let packed = packed_pnpm_manifest(root);
    assert_eq!(fs::read(&app_path).unwrap(), source_before);
    for (alias, prefix) in [
        ("empty", ""),
        ("star", ""),
        ("caret", "^"),
        ("tilde", "~"),
        ("path-lib", ""),
        ("pin", ""),
    ] {
        assert_eq!(
            packed["dependencies"][alias],
            format!("npm:@protocol/lib@{prefix}{version}")
        );
    }
    assert_eq!(packed["dependencies"]["range"], format!("npm:@protocol/lib@^{version}"));
    assert_eq!(
        packed["dependencies"]["covered"],
        format!("npm:@protocol/lib@{covered}")
    );
    assert_eq!(
        packed["optionalDependencies"]["optional"],
        format!("npm:@protocol/lib@~{version}")
    );
    let private = read_native_package(&root.join("packages/private-leaf/package.json"));
    assert_eq!(packed["optionalDependencies"]["private-leaf"], private["version"]);
    assert_eq!(packed["peerDependencies"]["@protocol/lib"], format!("^{version}"));
    assert_eq!(packed["peerDependenciesMeta"], app["peerDependenciesMeta"]);
}

#[test]
fn pnpm_workspace_protocols_pack_and_install_after_version() {
    if find_on_path("pnpm").is_none() {
        eprintln!("native coverage gap: pnpm is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    setup_pnpm_protocol_workspace(dir.path());
    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: dir.path().to_path_buf(),
        dry_run: false,
    };
    commands::add::handle(
        AddArgs {
            packages: vec!["@protocol/lib:minor".into()],
            summary: Some("Release workspace library".into()),
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
    assert_pnpm_protocols_and_pack(dir.path(), false);
}

#[test]
fn pnpm_workspace_protocols_pack_and_install_after_snapshot() {
    if find_on_path("pnpm").is_none() {
        eprintln!("native coverage gap: pnpm is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    setup_pnpm_protocol_workspace(dir.path());
    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: dir.path().to_path_buf(),
        dry_run: false,
    };
    commands::snapshot::handle(
        SnapshotArgs {
            tag: "canary".into(),
            strict: false,
        },
        &global,
    )
    .unwrap();
    assert_pnpm_protocols_and_pack(dir.path(), true);
}

#[test]
fn pnpm_workspace_protocol_requires_a_local_member() {
    if find_on_path("pnpm").is_none() {
        eprintln!("native coverage gap: pnpm is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    setup_pnpm_protocol_workspace(dir.path());
    let app_path = dir.path().join("packages/app/package.json");
    let mut app = read_native_package(&app_path);
    app["dependencies"]["missing"] = serde_json::json!("workspace:@protocol/missing@*");
    fs::write(&app_path, serde_json::to_vec_pretty(&app).unwrap()).unwrap();
    let output = Command::new("pnpm")
        .args(["install", "--lockfile-only", "--ignore-scripts", "--offline"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(diagnostic.contains("ERR_PNPM_WORKSPACE_PKG_NOT_FOUND"), "{diagnostic}");
}

#[test]
fn relative_workspace_reference_never_falls_back_to_an_admitted_same_name() {
    if find_on_path("pnpm").is_none() {
        eprintln!("native coverage gap: pnpm is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup_pnpm_protocol_workspace(root);
    fs::create_dir_all(root.join("outside-lib")).unwrap();
    fs::write(
        root.join("outside-lib/package.json"),
        r#"{"name":"@protocol/lib","version":"0.2.3"}"#,
    )
    .unwrap();
    let app_path = root.join("packages/app/package.json");
    let mut app = read_native_package(&app_path);
    app["dependencies"]["path-lib"] = serde_json::json!("workspace:../../outside-lib");
    fs::write(&app_path, serde_json::to_vec_pretty(&app).unwrap()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--format", "json", "--cwd", root.to_str().unwrap(), "status"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "callisto status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        status["diagnostics"].as_array().unwrap().iter().any(|diagnostic| {
            diagnostic["code"] == "unknown-package"
                && diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("admitted npm workspace member"))
        }),
        "relative reference incorrectly resolved by package name: {status}"
    );
}

#[cfg(unix)]
#[test]
fn relative_workspace_reference_resolves_when_workspace_root_is_a_symlink() {
    if find_on_path("pnpm").is_none() {
        eprintln!("native coverage gap: pnpm is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup_pnpm_protocol_workspace(root);
    let symlink_dir = tempfile::tempdir().unwrap();
    let symlink_root = symlink_dir.path().join("workspace-link");
    std::os::unix::fs::symlink(root, &symlink_root).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--format", "json", "--cwd", symlink_root.to_str().unwrap(), "status"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "callisto status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        !status["diagnostics"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|diagnostic| {
                diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("relative pnpm workspace dependency"))
            }),
        "valid relative dependency failed through a symlinked workspace root: {status}"
    );
}

#[test]
fn pnpm_workspace_shorthand_packs_current_target_versions() {
    if find_on_path("pnpm").is_none() {
        eprintln!("native coverage gap: pnpm is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    setup_pnpm_protocol_workspace(root);
    let app_path = root.join("packages/app/package.json");
    let mut app = read_native_package(&app_path);
    for (name, suffix) in [
        ("bare-empty", ""),
        ("bare-star", "*"),
        ("bare-caret", "^"),
        ("bare-tilde", "~"),
    ] {
        let package = root.join("packages").join(name);
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("package.json"),
            serde_json::to_vec(&serde_json::json!({"name": name, "version": "0.2.3"})).unwrap(),
        )
        .unwrap();
        app["dependencies"][name] = serde_json::json!(format!("workspace:{suffix}"));
    }
    fs::write(&app_path, serde_json::to_vec_pretty(&app).unwrap()).unwrap();
    run_pnpm(root, &["install", "--ignore-scripts", "--offline"]);
    let source_before = fs::read(&app_path).unwrap();
    let packed = packed_pnpm_manifest(root);
    assert_eq!(fs::read(&app_path).unwrap(), source_before);
    for (name, expected) in [
        ("bare-empty", "0.2.3"),
        ("bare-star", "0.2.3"),
        ("bare-caret", "^0.2.3"),
        ("bare-tilde", "~0.2.3"),
    ] {
        assert_eq!(packed["dependencies"][name], expected);
    }
    assert_eq!(packed["dependencies"]["range"], "npm:@protocol/lib@^0.2.0");
    assert_eq!(packed["dependencies"]["path-lib"], "npm:@protocol/lib@0.2.3");
}
