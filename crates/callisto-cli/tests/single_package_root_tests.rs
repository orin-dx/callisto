//! A repository whose root is one package (no workspace manifest) is a valid Callisto root.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use callisto_cli::cli::{
    AddArgs, GlobalArgs, InitArgs, InitVersioning, OutputFormat, ReleaseCommandArgs, StatusArgs, VersionArgs,
};
use callisto_cli::commands;
use callisto_fixtures::git::{init_repo, run_git};

fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    init_repo(dir.path());
    run_git(
        dir.path(),
        &["remote", "add", "origin", "https://github.com/example/single.git"],
    );
    run_git(dir.path(), &["add", "."]);
    run_git(dir.path(), &["commit", "-q", "-m", "initial"]);
    dir
}

fn global(cwd: &Path, dry_run: bool) -> GlobalArgs {
    GlobalArgs {
        format: OutputFormat::Json,
        cwd: cwd.to_path_buf(),
        dry_run,
    }
}

fn init(cwd: &Path) {
    let args = InitArgs {
        yes: true,
        versioning: Some(InitVersioning::Independent),
        ..Default::default()
    };
    assert_eq!(
        commands::init::handle(args, &global(cwd, false)).unwrap(),
        ExitCode::SUCCESS
    );
}

/// init, add, status (from a subdirectory), version, then release --dry-run.
fn lifecycle(dir: &tempfile::TempDir, package: &str, manifest: &str, bumped: &str) {
    let root = dir.path();
    init(&root.join("src"));
    assert!(root.join("callisto.toml").exists());
    assert!(!root.join("src/callisto.toml").exists());

    commands::add::handle(
        AddArgs {
            packages: vec![format!("{package}:minor")],
            summary: Some("Add a feature".to_owned()),
        },
        &global(root, false),
    )
    .unwrap();
    let status = StatusArgs {
        strict: false,
        strict_graph: false,
        check: false,
    };
    assert_eq!(
        commands::status::handle(status, &global(&root.join("src"), false)).unwrap(),
        ExitCode::SUCCESS
    );
    let version = VersionArgs {
        refresh_lockfiles: false,
        strict: false,
        strict_graph: false,
        allow_empty_changesets: false,
        emit_decision: None,
    };
    commands::version::handle(version, &global(root, false)).unwrap();
    assert!(fs::read_to_string(root.join(manifest)).unwrap().contains(bumped));

    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "version"]);
    let release = ReleaseCommandArgs {
        command: None,
        packages: vec![],
        receipt: None,
    };
    assert_eq!(
        commands::release::handle(release, &global(root, true)).unwrap(),
        ExitCode::SUCCESS
    );
}

#[test]
fn single_crate_repository() {
    let dir = repo(&[
        (
            "Cargo.toml",
            "[package]\nname = \"solo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "\n"),
    ]);
    lifecycle(&dir, "solo", "Cargo.toml", "version = \"0.2.0\"");
}

#[test]
fn single_npm_package_repository() {
    let dir = repo(&[
        (
            "package.json",
            "{\n  \"name\": \"solo\",\n  \"version\": \"0.1.0\"\n}\n",
        ),
        ("src/index.js", "\n"),
    ]);
    lifecycle(&dir, "solo", "package.json", "\"version\": \"0.2.0\"");
}

#[test]
fn single_python_package_repository_initializes() {
    let dir = repo(&[
        ("pyproject.toml", "[project]\nname = \"solo\"\nversion = \"0.1.0\"\n"),
        ("src/solo/__init__.py", "\n"),
    ]);
    init(&dir.path().join("src"));
    assert!(dir.path().join("callisto.toml").exists());
}
