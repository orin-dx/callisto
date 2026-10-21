//! What `version` writes must be something Git will stage, and `add` never replaces a changeset another run created.
#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;
use std::path::Path;
use std::process::Command;

use release_harness::{callisto, git};

/// A single crate whose `.gitignore` is `ignore`, committed.
fn crate_repo(ignore: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "\n").unwrap();
    fs::write(root.join(".gitignore"), ignore).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "initial"]);
    dir
}

fn add_changeset(root: &Path) {
    let add = callisto(root, &["add", "--package", "cargo/lib:minor", "--summary", "A feature"]);
    assert!(add.status.success(), "{}", String::from_utf8_lossy(&add.stderr));
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "changeset"]);
}

/// A library that ignores its `Cargo.lock` still releases: the lockfile is refreshed on disk and left unstaged.
#[test]
fn a_gitignored_lockfile_is_refreshed_but_never_staged() {
    let dir = crate_repo("target\nCargo.lock\n");
    let root = dir.path();
    add_changeset(root);

    let version = callisto(root, &["version"]);
    assert!(
        version.status.success(),
        "{}{}",
        String::from_utf8_lossy(&version.stdout),
        String::from_utf8_lossy(&version.stderr)
    );

    assert!(root.join("Cargo.lock").exists(), "the lockfile was still refreshed");
    let staged = git(root, &["diff", "--cached", "--name-only"]);
    assert!(staged.lines().any(|path| path == "Cargo.toml"), "{staged}");
    assert!(!staged.lines().any(|path| path == "Cargo.lock"), "{staged}");
}

/// A changelog a `.gitignore` ignores can never be committed, so `version` stops before it writes anything.
#[test]
fn a_gitignored_changelog_stops_the_run_with_nothing_changed() {
    let dir = crate_repo("target\nCHANGELOG.md\n");
    let root = dir.path();
    add_changeset(root);
    let before = git(root, &["status", "--porcelain"]);

    let version = callisto(root, &["version"]);
    assert!(!version.status.success());
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&version.stdout),
        String::from_utf8_lossy(&version.stderr)
    );
    assert!(
        output.contains("E314") && output.contains("ignored by a .gitignore"),
        "{output}"
    );

    assert!(fs::read_to_string(root.join("Cargo.toml"))
        .unwrap()
        .contains("version = \"0.1.0\""));
    assert!(!root.join("CHANGELOG.md").exists());
    assert_eq!(git(root, &["status", "--porcelain"]), before);
    assert_eq!(git(root, &["diff", "--cached", "--name-only"]), "");
}

/// Many `add` runs at once each keep their own changeset, however their names are drawn.
#[test]
fn concurrent_add_runs_never_replace_each_other() {
    let dir = crate_repo("target\n");
    let root = dir.path();
    let children: Vec<_> = (0..12)
        .map(|index| {
            Command::new(env!("CARGO_BIN_EXE_callisto"))
                .args([
                    "--cwd",
                    root.to_str().unwrap(),
                    "add",
                    "--package",
                    "cargo/lib:patch",
                    "--summary",
                ])
                .arg(format!("change number {index}"))
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }

    let changesets: Vec<_> = fs::read_dir(root.join(".changeset"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md") && !path.ends_with("README.md"))
        .collect();
    assert_eq!(changesets.len(), 12, "{changesets:?}");
    let bodies: String = changesets
        .iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .collect();
    for index in 0..12 {
        assert!(bodies.contains(&format!("change number {index}")), "missing {index}");
    }
}
