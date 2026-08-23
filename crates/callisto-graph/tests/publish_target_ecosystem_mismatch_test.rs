//! Regression test for the silent publish-target/ecosystem-mismatch bug:
//!
//! `PublishTarget` is a closed enum with variants for ecosystems callisto
//! does not actually dispatch to (`NuGet`, `GitHubRelease`). Before this fix,
//! `[[package]] publish-to = ["nuget"]` on an ordinary Cargo crate was
//! accepted silently at config-load / workspace-walk time: nothing validated
//! that the configured target's ecosystem matched the package's real,
//! detected ecosystem. The crate then silently dropped out of every actual
//! registry publish while the release-tag gate still fired, because that
//! gate only checked `publish_to` was non-empty and not all `None`.
//!
//! This test asserts that a Cargo-only package configured with a `nuget`
//! publish target fails workspace load with an explicit ecosystem-mismatch
//! error, instead of silently accepting the mismatched configuration.

use std::fs;
use std::path::Path;

use callisto_graph::error::GraphError;
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::Workspace;
use callisto_model::{CommandError, CommandOutput, CommandRunner};

struct NoopRunner;

impl CommandRunner for NoopRunner {
    fn run(&self, _program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
        Ok(CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

fn git_init(root: &Path) {
    std::process::Command::new("git")
        .arg("init")
        .current_dir(root)
        .output()
        .expect("git init should run");
}

/// A Cargo-only crate configured with `publish-to = ["nuget"]` (a NuGet
/// target — Ecosystem::NuGet, not Ecosystem::Cargo) must fail workspace load
/// with an explicit ecosystem-mismatch error rather than silently accepting
/// the mismatched target.
#[test]
fn cargo_package_with_mismatched_nuget_publish_target_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init(root);

    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"some-cargo-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "").unwrap();

    fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"some-cargo-crate\"\npublish-to = [\"nuget\"]\n",
    )
    .unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;

    let ws = Workspace::load(root.to_path_buf(), &locator, &runner);

    let err = match ws {
        Ok(_) => panic!(
            "workspace load must fail for a Cargo package configured with a NuGet \
             publish target (ecosystem mismatch); silently accepting it means the \
             crate is dropped from every real publish with zero diagnostic"
        ),
        Err(e) => e,
    };
    assert!(
        matches!(err, GraphError::PublishTargetEcosystemMismatch { .. }),
        "expected GraphError::PublishTargetEcosystemMismatch, got: {err:?}"
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("some-cargo-crate"),
        "error message should name the package: {msg}"
    );
}
