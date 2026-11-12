//! Usage errors exit 2 and runtime failures exit 1, for text and JSON output alike.
#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;
use std::process::Command;

use release_harness::*;

fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "workspace"]);
    dir
}

fn assert_exit(dir: &std::path::Path, args: &[&str], exit: i32, code: &str) {
    let json = callisto(dir, args);
    assert_eq!(json.status.code(), Some(exit), "json {args:?}: {}", stderr_of(&json));
    // `init` prints what it detected before the envelope.
    let stderr = stderr_of(&json);
    let body = stderr.find("{\n").map_or(stderr.as_str(), |start| &stderr[start..]);
    let envelope: serde_json::Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("{args:?} stderr has no JSON envelope ({error}): {stderr}"));
    assert_eq!(envelope["error"]["code"], code, "{args:?}: {envelope}");

    let text = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--cwd", dir.to_str().unwrap()])
        .args(args)
        .output()
        .unwrap();
    assert_eq!(text.status.code(), Some(exit), "text {args:?}: {}", stderr_of(&text));
}

#[test]
fn bad_or_missing_flag_values_checked_after_parsing_exit_2() {
    let dir = workspace();
    let root = dir.path();
    fs::write(root.join("bad.json"), "{").unwrap();
    for (args, code) in [
        (vec!["add", "--package", "a:patch", "--summary", "s"], "E317"),
        (vec!["add", "--package", "cargo/a:bogus", "--summary", "s"], "E271"),
        (vec!["add", "--package", "cargo/a:patch"], "E274"),
        (vec!["add", "--package", "cargo/a:patch", "--summary", ""], "E275"),
        (vec!["add", "--package", "cargo/nope:patch", "--summary", "s"], "E102"),
        (
            vec!["release", "--dry-run", "--package", "cargo/a", "--package", "cargo/a"],
            "E169",
        ),
        (
            vec!["release", "pending", "--decision", "d.json", "--from", &"a".repeat(40)],
            "E328",
        ),
        (vec!["compose-pr-body", "--preview", "bad.json"], "E281"),
        (vec!["add"], "E214"),
        (vec!["init"], "E240"),
        (vec!["init", "--workflow", "--no-workflow"], "E242"),
        (vec!["snapshot", "--tag", "a/b c"], "E324"),
        (vec!["pre", "enter", " "], "E279"),
    ] {
        assert_exit(root, &args, 2, code);
    }
}

#[test]
fn init_flag_values_checked_against_the_workspace_exit_2() {
    let dir = workspace();
    let root = dir.path();
    git(root, &["remote", "add", "origin", "https://github.com/o/r.git"]);
    let init = [
        "init",
        "--yes",
        "--artifact-target",
        "x86_64-unknown-linux-gnu",
        "--forge-repository",
    ];
    // A library has no binary to ship: workspace state, not a flag value.
    assert_exit(root, &[&init[..], &["o/r"]].concat(), 1, "E188");
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    assert_exit(root, &[&init[..], &["o/other"]].concat(), 2, "E187");
    assert_exit(
        root,
        &[&init[..], &["o/r", "--product-package", "nope"]].concat(),
        2,
        "E189",
    );
}

#[test]
fn arguments_rejected_while_parsing_exit_2_and_help_and_version_exit_0() {
    let dir = workspace();
    let root = dir.path();
    for args in [
        vec!["status", "--no-such-flag"],
        vec!["no-such-command"],
        vec!["--format", "yaml", "status"],
        vec!["pre", "enter"],
        vec!["--no-such-flag"],
    ] {
        for format in ["json", "text"] {
            let out = Command::new(env!("CARGO_BIN_EXE_callisto"))
                .args(["--format", format, "--cwd", root.to_str().unwrap()])
                .args(&args)
                .output()
                .unwrap();
            assert_eq!(out.status.code(), Some(2), "{format} {args:?}: {}", stderr_of(&out));
        }
    }
    for args in [vec!["--help"], vec!["status", "--help"], vec!["--version"]] {
        let out = Command::new(env!("CARGO_BIN_EXE_callisto"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", stderr_of(&out));
    }
}

#[test]
fn runtime_failures_still_exit_1() {
    let dir = workspace();
    let root = dir.path();
    fs::write(root.join("callisto.toml"), "not = [valid").unwrap();
    assert_exit(root, &["status"], 1, "E111");
}

#[test]
fn a_bare_package_in_config_exits_1_though_the_same_code_on_a_flag_exits_2() {
    let dir = workspace();
    let root = dir.path();
    fs::write(root.join("callisto.toml"), "[[package]]\nmatch = \"a\"\n").unwrap();
    assert_exit(root, &["status"], 1, "E317");
}

#[test]
fn version_emit_decision_outside_the_workspace_exits_2_before_writing() {
    let dir = workspace();
    let root = dir.path();
    let outside = tempfile::tempdir().unwrap();
    let escaped = outside.path().join("decision.json");
    let relative = format!(
        "../{}/decision.json",
        outside.path().file_name().unwrap().to_str().unwrap()
    );
    assert!(callisto(root, &["add", "--package", "cargo/a:patch", "--summary", "s"])
        .status
        .success());
    for path in [escaped.to_str().unwrap(), relative.as_str()] {
        let out = callisto(root, &["version", "--emit-decision", path]);
        assert_eq!(out.status.code(), Some(2), "{path}: {}", stderr_of(&out));
        assert!(!escaped.exists(), "{path} wrote outside the workspace");
    }
    assert!(fs::read_to_string(root.join("Cargo.toml")).unwrap().contains("0.1.0"));
}

#[test]
fn version_emit_decision_to_an_absolute_path_whose_directories_do_not_exist_yet_still_writes_it() {
    let dir = workspace();
    let root = dir.path();
    assert!(callisto(root, &["add", "--package", "cargo/a:patch", "--summary", "s"])
        .status
        .success());
    let decision = root.join(".callisto/nested/release-decision.json");
    let out = callisto(root, &["version", "--emit-decision", decision.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    assert!(decision.exists());
}

#[test]
fn release_pending_decision_outside_the_workspace_exits_2() {
    let dir = workspace();
    let outside = tempfile::tempdir().unwrap();
    let escaped = outside.path().join("decision.json");
    assert_exit(dir.path(), &["release", "pending", "--decision", "../x"], 2, "E226");
    assert_exit(
        dir.path(),
        &["release", "pending", "--decision", escaped.to_str().unwrap()],
        2,
        "E225",
    );
}

/// Each target must be refused with exit 2 before the plan is applied, leaving the manifest and changesets untouched.
fn assert_decision_target_rejected(
    setup: impl Fn(&std::path::Path),
    target: impl Fn(&std::path::Path) -> String,
    code: &str,
) {
    let dir = workspace();
    let root = dir.path();
    setup(root);
    assert!(callisto(root, &["add", "--package", "cargo/a:patch", "--summary", "s"])
        .status
        .success());
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let changesets = fs::read_dir(root.join(".changeset")).unwrap().count();
    let target = target(root);
    assert_exit(root, &["version", "--emit-decision", &target], 2, code);
    assert_eq!(
        fs::read_to_string(root.join("Cargo.toml")).unwrap(),
        manifest,
        "{target}"
    );
    assert_eq!(
        fs::read_dir(root.join(".changeset")).unwrap().count(),
        changesets,
        "{target}"
    );
}

#[test]
fn version_emit_decision_to_a_directory_or_the_root_exits_2_before_applying() {
    assert_decision_target_rejected(|_| {}, |_| ".changeset".to_owned(), "E226");
    assert_decision_target_rejected(|_| {}, |_| ".".to_owned(), "E226");
    assert_decision_target_rejected(|_| {}, |root| root.to_str().unwrap().to_owned(), "E226");
    assert_decision_target_rejected(
        |_| {},
        |root| root.join(".changeset").to_str().unwrap().to_owned(),
        "E226",
    );
}

#[test]
fn version_emit_decision_with_a_trailing_slash_or_inside_git_exits_2_before_applying() {
    assert_decision_target_rejected(|_| {}, |_| "out/".to_owned(), "E226");
    assert_decision_target_rejected(|_| {}, |_| ".git/d.json".to_owned(), "E226");
    assert_decision_target_rejected(|_| {}, |root| format!("{}/missing/../d.json", root.display()), "E226");
}

#[cfg(unix)]
#[test]
fn version_emit_decision_through_a_symlink_out_of_the_workspace_exits_2_before_applying() {
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().to_path_buf();
    assert_decision_target_rejected(
        |root| std::os::unix::fs::symlink(&target, root.join("link")).unwrap(),
        |_| "link/d.json".to_owned(),
        "E225",
    );
    assert!(!outside.path().join("d.json").exists());
}

#[test]
fn version_emit_decision_to_a_relative_path_whose_directories_do_not_exist_yet_still_writes_it() {
    let dir = workspace();
    let root = dir.path();
    assert!(callisto(root, &["add", "--package", "cargo/a:patch", "--summary", "s"])
        .status
        .success());
    let out = callisto(
        root,
        &["version", "--emit-decision", ".callisto/nested/release-decision.json"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr_of(&out));
    assert!(root.join(".callisto/nested/release-decision.json").exists());
}
