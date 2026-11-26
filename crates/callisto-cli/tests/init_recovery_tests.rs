#![cfg(unix)]

//! A failed `init` leaves a workspace the advice in its error recovers.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Output};

use callisto_fixtures::git::{init_repo, run_git};

fn init(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args([
            "--cwd",
            root.to_str().unwrap(),
            "init",
            "--yes",
            "--versioning",
            "independent",
        ])
        .output()
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A `.changeset/README.md` that cannot be written fails `init` before `callisto.toml` exists, so once the directory
/// is writable again a plain `callisto init` completes it.
#[test]
fn a_failed_readme_write_leaves_no_config_and_a_retry_completes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"solo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial"]);
    let changesets = root.join(".changeset");
    fs::create_dir_all(&changesets).unwrap();
    fs::set_permissions(&changesets, fs::Permissions::from_mode(0o555)).unwrap();

    let failed = init(root);
    fs::set_permissions(&changesets, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(failed.status.code(), Some(1), "{}", stderr(&failed));
    assert!(stderr(&failed).contains("E278"), "{}", stderr(&failed));
    assert!(!root.join("callisto.toml").exists());
    assert!(!changesets.join("README.md").exists());

    let retried = init(root);
    assert!(retried.status.success(), "{}", stderr(&retried));
    assert!(root.join("callisto.toml").exists());
    assert!(changesets.join("README.md").exists());
}
