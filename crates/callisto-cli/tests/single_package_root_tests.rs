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
        check: false,
    };
    assert_eq!(
        commands::status::handle(status, &global(&root.join("src"), false)).unwrap(),
        ExitCode::SUCCESS
    );
    let version = VersionArgs {
        refresh_lockfiles: false,
        no_refresh_lockfiles: true,
        strict: false,
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

#[test]
fn root_package_release_commit_accepts_default_and_configured_changelog_paths() {
    let cases = [
        (
            "cargo/solo",
            "crates-io",
            "Cargo.toml",
            "[package]\nname = \"solo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            "src/lib.rs",
        ),
        (
            "npm/solo",
            "npm",
            "package.json",
            "{\"name\":\"solo\",\"version\":\"0.1.0\"}\n",
            "src/index.js",
        ),
        (
            "pypi/solo",
            "pypi",
            "pyproject.toml",
            "[project]\nname = \"solo\"\nversion = \"0.1.0\"\n",
            "src/solo/__init__.py",
        ),
    ];
    for (identity, registry, manifest, source, code) in cases {
        for custom_changelog in [false, true] {
            let dir = repo(&[(manifest, source), (code, "\n")]);
            let root = dir.path();
            init(root);
            let config = fs::read_to_string(root.join("callisto.toml")).unwrap();
            let changelog_config = if custom_changelog {
                "changelog = \"./notes/../RELEASES.md\"\n"
            } else {
                ""
            };
            callisto_model::atomic::atomic_write(
                &root.join("callisto.toml"),
                &format!(
                    "{config}\n[[package]]\nmatch = \"{identity}\"\npublish-to = [\"{registry}\"]\n\
                     {changelog_config}",
                ),
                &callisto_model::ApplyPermit::force_for_tests(),
            )
            .unwrap();
            commands::add::handle(
                AddArgs {
                    packages: vec!["solo:patch".to_owned()],
                    summary: Some("Fix the root package".to_owned()),
                },
                &global(root, false),
            )
            .unwrap();
            run_git(root, &["add", "."]);
            run_git(root, &["commit", "-q", "-m", "configure and request release"]);
            let decision_path = ".callisto/release-decision.json";
            commands::version::handle(
                VersionArgs {
                    refresh_lockfiles: false,
                    no_refresh_lockfiles: true,
                    strict: false,
                    allow_empty_changesets: false,
                    emit_decision: Some(decision_path.into()),
                },
                &global(root, false),
            )
            .unwrap();
            let changelog_path = if custom_changelog {
                "RELEASES.md"
            } else {
                "CHANGELOG.md"
            };
            assert!(root.join(changelog_path).exists());
            run_git(root, &["add", "."]);
            run_git(root, &["commit", "-q", "-m", "release root package"]);
            let head = std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(root)
                .output()
                .unwrap();
            assert!(head.status.success());
            let commit = String::from_utf8(head.stdout).unwrap().trim().to_owned();
            run_git(root, &["checkout", "--detach", &commit]);
            let external = tempfile::tempdir().unwrap();
            let intent_path = external.path().join("intent.json");
            let out = std::process::Command::new(env!("CARGO_BIN_EXE_callisto"))
                .args([
                    "--cwd",
                    root.to_str().unwrap(),
                    "release",
                    "plan",
                    "--from-release-commit",
                    &commit,
                ])
                .args(["--decision", decision_path, "--out", intent_path.to_str().unwrap()])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{identity}, custom changelog {custom_changelog}: {}",
                String::from_utf8_lossy(&out.stderr),
            );
            let intent: callisto_model::ReleaseIntentV1 =
                serde_json::from_slice(&fs::read(intent_path).unwrap()).unwrap();
            assert_eq!(intent.decision.entries.len(), 1);
            assert_eq!(intent.decision.entries[0].package.to_string(), identity);
            assert_eq!(intent.decision.entries[0].target_version.render(), "0.1.1");
        }
    }
}
