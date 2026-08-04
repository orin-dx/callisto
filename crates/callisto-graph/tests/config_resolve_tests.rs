/// Tests that `[[linked-group]]` and `[[fixed-group]]` blocks in callisto.toml are
/// resolved into the workspace's `config.groups` table.
///
/// Regression test for the bug where `GroupTable::resolve()` was never called —
/// `raw_groups` was validated syntactically but then dropped, and
/// `ResolvedConfig.groups` was always set to `GroupTable::default()`.
use std::fs;
use std::path::Path;

use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::Workspace;
use callisto_model::{CommandError, CommandOutput, CommandRunner};

struct NoopRunner;

impl CommandRunner for NoopRunner {
    fn run(
        &self,
        _program: &str,
        _args: &[&str],
        _cwd: &Path,
    ) -> Result<CommandOutput, CommandError> {
        Ok(CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

/// Set up a minimal two-package Cargo workspace with git init.
fn write_two_crate_workspace(root: &Path, crate_a: &str, crate_b: &str) {
    std::process::Command::new("git")
        .arg("init")
        .current_dir(root)
        .output()
        .expect("git init should run");

    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    )
    .unwrap();

    for name in [crate_a, crate_b] {
        let crate_dir = root.join(format!("crates/{name}"));
        fs::create_dir_all(&crate_dir).unwrap();
        fs::write(
            crate_dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
    }
}

/// A `[[linked-group]]` defined in `callisto.toml` must appear in
/// `ws.config.groups.linked` after `Workspace::load`.
///
/// Before the fix this assertion failed because `GroupTable::resolve` was
/// never called and `groups` was always `GroupTable::default()` (empty maps).
#[test]
fn linked_group_is_resolved_into_config() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_two_crate_workspace(root, "alpha", "beta");

    fs::write(
        root.join("callisto.toml"),
        r#"
[[linked-group]]
name = "ab"
members = ["alpha", "beta"]
"#,
    )
    .unwrap();

    let runner = NoopRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace should load");

    assert!(
        ws.config.groups.linked.keys().any(|k| k.as_str() == "ab"),
        "expected linked group 'ab' to be present, but groups.linked was empty — \
         GroupTable::resolve was not called (bug)"
    );
}

/// A `[[fixed-group]]` defined in `callisto.toml` must appear in
/// `ws.config.groups.fixed` after `Workspace::load`.
#[test]
fn fixed_group_is_resolved_into_config() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_two_crate_workspace(root, "foo", "bar");

    fs::write(
        root.join("callisto.toml"),
        r#"
[[fixed-group]]
name = "fb"
members = ["foo", "bar"]
"#,
    )
    .unwrap();

    let runner = NoopRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace should load");

    assert!(
        ws.config.groups.fixed.keys().any(|k| k.as_str() == "fb"),
        "expected fixed group 'fb' to be present, but groups.fixed was empty — \
         GroupTable::resolve was not called (bug)"
    );
}
