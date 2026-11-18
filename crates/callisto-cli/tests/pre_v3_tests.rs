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
