use std::fs;
use std::process::Command;
use tempfile::tempdir;

/// Creates a temporary directory with a minimal polyglot git repository:
/// - a Cargo workspace with one crate (`core-crate` at 0.1.0)
/// - an npm `package.json` (`@myorg/web-app` at 1.0.0)
/// - an initial git commit
///
/// The directory is returned as a `TempDir`; dropping it cleans up disk.
pub fn setup_polyglot_git_repo() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let root = dir.path();

    let status = Command::new("git")
        .args(["init"])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success());

    Command::new("git")
        .args(["config", "user.name", "Callisto Tester"])
        .current_dir(root)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "tester@callisto.dev"])
        .current_dir(root)
        .status()
        .unwrap();

    let cargo_toml = r#"[workspace]
members = ["crates/core"]
resolver = "2"
"#;
    fs::write(root.join("Cargo.toml"), cargo_toml).unwrap();

    fs::create_dir_all(root.join("crates/core/src")).unwrap();
    let crate_toml = r#"[package]
name = "core-crate"
version = "0.1.0"
edition = "2021"
"#;
    fs::write(root.join("crates/core/Cargo.toml"), crate_toml).unwrap();
    fs::write(root.join("crates/core/src/lib.rs"), "pub fn hello() {}\n").unwrap();

    fs::create_dir_all(root.join("packages/web")).unwrap();
    let pkg_json = r#"{
  "name": "@myorg/web-app",
  "version": "1.0.0"
}
"#;
    fs::write(root.join("packages/web/package.json"), pkg_json).unwrap();

    Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(root)
        .status()
        .unwrap();

    dir
}
