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

/// `version` stopped after archiving: its writes are on disk, nothing is committed.
fn uncommitted_version(root: &Path) {
    let out = callisto(root, &["version", "--no-refresh-lockfiles"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
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

/// The same holds when a name clash archived the note under a suffixed name.
#[test]
fn a_note_left_beside_its_suffixed_archived_copy_is_not_applied_twice() {
    let dir = v3_cycle();
    let root = dir.path();
    add_changeset(root, "feature", "minor");
    version(root);
    add_changeset(root, "feature", "patch");
    uncommitted_version(root);
    assert_eq!(manifest_version(root), "1.1.0-beta.1");
    fs::copy(
        root.join(".changeset/pre/feature-2.md"),
        root.join(".changeset/feature.md"),
    )
    .unwrap();

    let rerun = callisto(root, &["version", "--no-refresh-lockfiles"]);
    assert!(rerun.status.success(), "{}", String::from_utf8_lossy(&rerun.stderr));
    assert_eq!(manifest_version(root), "1.1.0-beta.1");
    assert_eq!(markdown_in(&root.join(".changeset")), Vec::<String>::new());
    assert_eq!(
        markdown_in(&root.join(".changeset/pre")),
        ["feature-2.md", "feature.md"]
    );
}

/// At exit an archived note and a new note of the same name are two changes, each with its own summary.
#[test]
fn an_archived_note_and_a_new_note_of_the_same_name_both_reach_the_release_preview() {
    let dir = v3_cycle();
    let root = dir.path();
    fs::write(
        root.join(".changeset/feature.md"),
        "---\ncargo/pkg-alpha: minor\n---\n\nfeature\n",
    )
    .unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "Add a feature (#12)"]);
    version(root);
    let exit = callisto(root, &["pre", "exit"]);
    assert!(exit.status.success(), "{}", String::from_utf8_lossy(&exit.stderr));
    fs::write(
        root.join(".changeset/feature.md"),
        "---\ncargo/pkg-alpha: patch\n---\n\nsecond note\n",
    )
    .unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "exit with a new note"]);

    let preview = root.join("preview.json");
    let out = callisto(
        root,
        &[
            "version",
            "--no-refresh-lockfiles",
            "--emit-preview",
            preview.to_str().unwrap(),
        ],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = fs::read_to_string(&preview).unwrap();
    assert!(text.contains("second note"), "{text}");
    assert!(text.contains("\"summary\": \"feature\""), "{text}");
    fn change_named<'a>(value: &'a serde_json::Value, filename: &str) -> Option<&'a serde_json::Value> {
        match value {
            serde_json::Value::Object(map) if map.get("filename").is_some_and(|f| f == filename) => Some(value),
            serde_json::Value::Object(map) => map.values().find_map(|v| change_named(v, filename)),
            serde_json::Value::Array(items) => items.iter().find_map(|v| change_named(v, filename)),
            _ => None,
        }
    }
    let preview: serde_json::Value = serde_json::from_str(&text).unwrap();
    let archived = change_named(&preview, "pre/feature").expect("the archived note is a change of its own");
    assert_eq!(archived["pr"], 12, "{text}");
}

/// A note identical to one this cycle already archived is a duplicate: it is removed, and the run says so.
#[test]
fn a_duplicate_of_an_archived_note_is_removed_with_a_note() {
    let dir = v3_cycle();
    let root = dir.path();
    add_changeset(root, "feature", "minor");
    version(root);
    add_changeset(root, "feature", "minor");
    let out = callisto(root, &["version", "--no-refresh-lockfiles"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains(".changeset/feature.md") && stderr.contains(".changeset/pre/feature.md"),
        "{stderr}"
    );
    assert_eq!(manifest_version(root), "1.1.0-beta.0");
    assert_eq!(markdown_in(&root.join(".changeset")), Vec::<String>::new());
    assert_eq!(markdown_in(&root.join(".changeset/pre")), ["feature.md"]);
}

/// A run that archived a note but stopped before deleting it is finished on rerun, not applied again.
#[test]
fn a_note_left_beside_its_archived_copy_is_not_applied_twice() {
    let dir = v3_cycle();
    let root = dir.path();
    add_changeset(root, "feature", "minor");
    uncommitted_version(root);
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
    add_changeset(root, "feature", "major");
    version(root);
    assert_eq!(manifest_version(root), "2.0.0-beta.0");
    assert_eq!(
        markdown_in(&root.join(".changeset/pre")),
        ["feature-2.md", "feature-3.md", "feature.md"]
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

    // The run had already deleted the archive from disk: `HEAD` still holds it, so the run is still refused.
    fs::remove_dir_all(root.join(".changeset/pre")).unwrap();
    let rerun = callisto(root, &["version", "--no-refresh-lockfiles"]);
    let stderr = String::from_utf8_lossy(&rerun.stderr);
    assert!(!rerun.status.success(), "{stderr}");
    assert!(stderr.contains("E121"), "{stderr}");
}

/// A version edited by hand with nothing pending, archive included, is not an interrupted run.
#[test]
fn a_hand_edited_version_with_nothing_pending_is_not_refused() {
    let dir = v3_cycle();
    let root = dir.path();
    let exit = callisto(root, &["pre", "exit"]);
    assert!(exit.status.success(), "{}", String::from_utf8_lossy(&exit.stderr));
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "exit pre"]);
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    fs::write(root.join("Cargo.toml"), manifest.replace("1.0.0", "1.0.5")).unwrap();

    let run = callisto(root, &["version", "--no-refresh-lockfiles"]);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!stderr.contains("E121"), "{stderr}");
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

/// An unparseable archived note fails `status --check`, as an unparseable new one does.
#[test]
fn status_check_reports_an_unparseable_archived_note() {
    let dir = v3_cycle();
    let root = dir.path();
    fs::create_dir_all(root.join(".changeset/pre")).unwrap();
    fs::write(
        root.join(".changeset/pre/broken.md"),
        "---\ncargo/pkg-alpha: sideways\n---\n\nBroken.\n",
    )
    .unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "broken archive"]);

    let status = callisto(root, &["--format", "json", "status", "--check"]);
    assert_eq!(
        status.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    let failures: Vec<&str> = report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "changeset-parse-failed")
        .map(|d| d["path"].as_str().unwrap())
        .collect();
    assert_eq!(failures, [".changeset/pre/broken.md"]);
}

/// `pre exit` releases the archive and deletes pre.json, so it refuses while an archived note names a removed package.
#[test]
fn exit_refuses_while_an_archived_note_names_a_removed_package() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"a\", \"b\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    for name in ["a", "b"] {
        fs::create_dir_all(root.join(name).join("src")).unwrap();
        fs::write(
            root.join(name).join("Cargo.toml"),
            format!("[package]\nname = \"pkg-{name}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        fs::write(root.join(name).join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    }
    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(
        root.join(".changeset/pre.json"),
        "{\"mode\":\"pre\",\"tag\":\"beta\"}\n",
    )
    .unwrap();
    fs::write(
        root.join(".changeset/both.md"),
        "---\ncargo/pkg-a: minor\ncargo/pkg-b: minor\n---\n\nBoth.\n",
    )
    .unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "enter pre"]);
    version(root);
    assert_eq!(markdown_in(&root.join(".changeset/pre")), ["both.md"]);

    fs::remove_dir_all(root.join("b")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"a\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    let exit = callisto(root, &["pre", "exit"]);
    assert!(exit.status.success(), "{}", String::from_utf8_lossy(&exit.stderr));
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "drop b and exit"]);

    let run = callisto(root, &["version", "--no-refresh-lockfiles"]);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!run.status.success(), "{stderr}");
    assert!(
        stderr.contains("E102") && stderr.contains("pkg-b") && stderr.contains("both.md"),
        "{stderr}"
    );
    assert!(root.join(".changeset/pre.json").exists());
    assert_eq!(markdown_in(&root.join(".changeset/pre")), ["both.md"]);
}

/// A run that archived its note and then failed writing the decision resumes without bumping or logging twice.
#[test]
fn a_run_interrupted_after_archiving_resumes() {
    let dir = v3_cycle();
    let root = dir.path();
    add_changeset(root, "feature", "minor");
    fs::write(root.join("blocked"), "a file where the preview's directory should be\n").unwrap();
    let args = [
        "version",
        "--no-refresh-lockfiles",
        "--emit-decision",
        ".callisto/release-decision.json",
        "--emit-preview",
        "blocked/preview.json",
    ];

    let failed = callisto(root, &args);
    assert!(!failed.status.success(), "the preview write must fail");
    assert_eq!(markdown_in(&root.join(".changeset/pre")), ["feature.md"]);
    assert!(!root.join(".changeset/feature.md").exists());

    fs::remove_file(root.join("blocked")).unwrap();
    let resumed = callisto(root, &args);
    assert!(resumed.status.success(), "{}", String::from_utf8_lossy(&resumed.stderr));
    assert_eq!(manifest_version(root), "1.1.0-beta.0");
    assert!(root.join(".callisto/release-decision.json").exists());
    let changelog = fs::read_to_string(root.join("CHANGELOG.md")).unwrap_or_default();
    assert!(changelog.matches("## 1.1.0-beta.0").count() <= 1, "{changelog}");
}

/// A new note naming an unknown package stays on disk with a warning at exit; only an archived one blocks it.
#[test]
fn exit_keeps_warning_about_a_new_note_that_names_an_unknown_package() {
    let dir = v3_cycle();
    let root = dir.path();
    let exit = callisto(root, &["pre", "exit"]);
    assert!(exit.status.success(), "{}", String::from_utf8_lossy(&exit.stderr));
    fs::write(
        root.join(".changeset/ghost.md"),
        "---\ncargo/ghost: minor\n---\n\nNames a package that does not exist.\n",
    )
    .unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "exit with a stray note"]);

    let run = callisto(root, &["version", "--no-refresh-lockfiles"]);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    assert!(root.join(".changeset/ghost.md").exists());
}

/// An archived note whose bare name becomes ambiguous mid-cycle fails `status --check`, as `version` would fail.
#[test]
fn status_check_reports_an_archived_note_whose_name_became_ambiguous() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    fs::write(root.join("callisto.toml"), "").unwrap();
    fs::create_dir_all(root.join("core/src")).unwrap();
    fs::write(
        root.join("core/Cargo.toml"),
        "[package]\nname = \"foo\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("core/src/lib.rs"), "pub fn f() {}\n").unwrap();
    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(
        root.join(".changeset/pre.json"),
        "{\"mode\":\"pre\",\"tag\":\"beta\"}\n",
    )
    .unwrap();
    fs::write(root.join(".changeset/bare.md"), "---\nfoo: minor\n---\n\nA feature.\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "enter pre"]);
    version(root);
    assert_eq!(markdown_in(&root.join(".changeset/pre")), ["bare.md"]);

    fs::create_dir_all(root.join("web")).unwrap();
    fs::write(
        root.join("web/package.json"),
        "{\"name\":\"foo\",\"version\":\"1.0.0\"}\n",
    )
    .unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "an npm foo arrives"]);

    let status = callisto(root, &["--format", "json", "status", "--check"]);
    assert_eq!(
        status.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    let ambiguous: Vec<&str> = report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "ambiguous-package-name")
        .map(|d| d["path"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ambiguous, [".changeset/pre/bare.md"], "{report}");
}
