//! A Changesets v3 pre cycle (`pre.json` holds only `mode` and `tag`; applied notes live in `.changeset/pre/`)
//! driven through the real binary.

use std::fs;
use std::path::Path;
use std::process::Command;

use callisto_fixtures::git::{init_repo, run_git};

fn callisto(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--cwd", root.to_str().unwrap()])
        .args(args)
        .output()
        .unwrap()
}

fn version(root: &Path) {
    let out = callisto(root, &["version", "--no-refresh-lockfiles"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "version"]);
}

fn manifest_version(root: &Path) -> String {
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let line = manifest.lines().find(|line| line.starts_with("version")).unwrap();
    line.trim_start_matches("version = ").trim_matches('"').to_string()
}

fn markdown_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .filter(|name| name.ends_with(".md") && name != "README.md")
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn add_changeset(root: &Path, id: &str, bump: &str) {
    fs::write(
        root.join(format!(".changeset/{id}.md")),
        format!("---\ncargo/pkg-alpha: {bump}\n---\n\n{id}\n"),
    )
    .unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", id]);
}

/// One Cargo package at 1.0.0 in a committed v3 pre cycle.
fn v3_cycle() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"pkg-alpha\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(
        root.join(".changeset/pre.json"),
        "{\"mode\":\"pre\",\"tag\":\"beta\"}\n",
    )
    .unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "enter pre"]);
    dir
}

/// A run that archived a note but stopped before deleting it is finished on rerun, not applied again.
#[test]
fn a_note_left_beside_its_archived_copy_is_not_applied_twice() {
    let dir = v3_cycle();
    let root = dir.path();
    add_changeset(root, "feature", "minor");
    version(root);
    fs::copy(
        root.join(".changeset/pre/feature.md"),
        root.join(".changeset/feature.md"),
    )
    .unwrap();

    let rerun = callisto(root, &["version", "--no-refresh-lockfiles"]);
    assert!(rerun.status.success(), "{}", String::from_utf8_lossy(&rerun.stderr));
    assert_eq!(manifest_version(root), "1.1.0-beta.0");
    assert_eq!(markdown_in(&root.join(".changeset")), Vec::<String>::new());
    assert_eq!(markdown_in(&root.join(".changeset/pre")), ["feature.md"]);
}

/// A different note under an archived note's name is archived beside it, never over it.
#[test]
fn a_new_note_with_an_archived_name_is_archived_under_a_free_name() {
    let dir = v3_cycle();
    let root = dir.path();
    add_changeset(root, "feature", "minor");
    version(root);
    add_changeset(root, "feature", "patch");
    version(root);
    assert_eq!(manifest_version(root), "1.1.0-beta.1");
    assert_eq!(
        markdown_in(&root.join(".changeset/pre")),
        ["feature-2.md", "feature.md"]
    );
    assert!(fs::read_to_string(root.join(".changeset/pre/feature.md"))
        .unwrap()
        .contains("minor"));
}

/// After `pre exit`, a version written to disk but not committed is an interrupted run while the archive is pending.
#[test]
fn an_interrupted_exit_run_is_refused_while_the_archive_is_pending() {
    let dir = v3_cycle();
    let root = dir.path();
    add_changeset(root, "feature", "minor");
    version(root);
    let exit = callisto(root, &["pre", "exit"]);
    assert!(exit.status.success(), "{}", String::from_utf8_lossy(&exit.stderr));
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "exit pre"]);
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    fs::write(root.join("Cargo.toml"), manifest.replace("1.1.0-beta.0", "1.1.0")).unwrap();

    let rerun = callisto(root, &["version", "--no-refresh-lockfiles"]);
    let stderr = String::from_utf8_lossy(&rerun.stderr);
    assert!(!rerun.status.success(), "{stderr}");
    assert!(stderr.contains("E121"), "{stderr}");
}

#[test]
fn a_v3_pre_cycle_archives_applied_changesets_and_releases_them_at_exit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"pkg-alpha\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(
        root.join(".changeset/pre.json"),
        "{\n  \"mode\": \"pre\",\n  \"tag\": \"beta\"\n}\n",
    )
    .unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "enter pre"]);

    add_changeset(root, "feature", "minor");
    version(root);
    assert_eq!(manifest_version(root), "1.1.0-beta.0");
    assert_eq!(markdown_in(&root.join(".changeset")), Vec::<String>::new());
    assert_eq!(markdown_in(&root.join(".changeset/pre")), ["feature.md"]);
    assert_eq!(
        fs::read_to_string(root.join(".changeset/pre.json")).unwrap(),
        "{\n  \"mode\": \"pre\",\n  \"tag\": \"beta\"\n}\n",
        "a v3 pre.json stays v3"
    );

    let rerun = callisto(root, &["version", "--no-refresh-lockfiles"]);
    assert!(rerun.status.success(), "{}", String::from_utf8_lossy(&rerun.stderr));
    assert_eq!(
        manifest_version(root),
        "1.1.0-beta.0",
        "an archived changeset is not applied again"
    );

    add_changeset(root, "fix", "patch");
    version(root);
    assert_eq!(manifest_version(root), "1.1.0-beta.1");
    assert_eq!(markdown_in(&root.join(".changeset/pre")), ["feature.md", "fix.md"]);

    let exit = callisto(root, &["pre", "exit"]);
    assert!(exit.status.success(), "{}", String::from_utf8_lossy(&exit.stderr));
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "exit pre"]);
    version(root);
    assert_eq!(manifest_version(root), "1.1.0");
    assert_eq!(markdown_in(&root.join(".changeset/pre")), Vec::<String>::new());
    assert!(!root.join(".changeset/pre.json").exists());
}
