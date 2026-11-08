#![cfg(unix)]

//! Every path-valued flag of `release` and `release-pr` is classified in one table, and every output
//! flag honors the shared rules: `--dry-run` refuses (or ignores) it before writing, and a relative
//! path is relative to `--cwd`, not the process directory.

#[allow(dead_code)]
mod common;
#[path = "common/release_harness.rs"]
mod release_harness;

use std::any::TypeId;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use clap::CommandFactory;
use release_harness::*;

#[derive(Clone, Copy)]
enum Role {
    /// Written by the command; `--dry-run` outcome and E-code.
    Output(DryRun),
    /// Read, relative to `--cwd`.
    Input,
    /// Read or built in, relative to `--cwd`.
    Dir,
    /// A committed decision file, relative to the workspace root; out of scope for the `--cwd` rule.
    WorkspaceDecision,
}

#[derive(Clone, Copy)]
enum DryRun {
    Refused(&'static str),
    /// The command previews and never reaches the write.
    Ignored,
}

use DryRun::{Ignored, Refused};
use Role::{Dir, Input, Output as Out, WorkspaceDecision};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// The one table: flag, role, and the other arguments that reach the write.
const FLAGS: &[(&str, Role, &[&str])] = &[
    ("release --receipt", Out(Ignored), &["release"]),
    ("release pending --decision", WorkspaceDecision, &[]),
    ("release plan --source-root", Dir, &[]),
    ("release plan --decision", WorkspaceDecision, &[]),
    (
        "release plan --out",
        Out(Refused("E217")),
        &["release", "plan", "--package", "cargo/x"],
    ),
    ("release inspect --input", Input, &[]),
    ("release build-python --source-root", Dir, &[]),
    ("release build-python --intent", Input, &[]),
    ("release build-python --artifact-dir", Dir, &[]),
    ("release artifact-manifest --intent", Input, &[]),
    ("release artifact-manifest --artifact-dir", Dir, &[]),
    (
        "release artifact-manifest --out",
        Out(Refused("E216")),
        &[
            "release",
            "artifact-manifest",
            "--intent",
            "i.json",
            "--artifact-dir",
            ".",
        ],
    ),
    ("release execute --source-root", Dir, &[]),
    ("release execute --intent", Input, &[]),
    ("release execute --artifact-manifest", Input, &[]),
    ("release execute --artifact-dir", Dir, &[]),
    (
        "release execute --receipt",
        Out(Refused("E218")),
        &[
            "release",
            "execute",
            "--intent",
            "i.json",
            "--orchestration-revision",
            SHA,
        ],
    ),
    (
        "release-pr commit-plan --out",
        Out(Refused("E282")),
        &["release-pr", "commit-plan", "--base-commit", SHA, "--message", "m"],
    ),
];

fn collect(command: &clap::Command, path: &str, found: &mut BTreeSet<String>) {
    for arg in command.get_arguments() {
        let Some(long) = arg.get_long() else { continue };
        if arg.is_global_set() || !(arg.get_value_parser().type_id() == TypeId::of::<PathBuf>()) {
            continue;
        }
        found.insert(format!("{path} --{long}"));
    }
    for sub in command.get_subcommands() {
        collect(sub, &format!("{path} {}", sub.get_name()), found);
    }
}

#[test]
fn every_path_flag_of_release_and_release_pr_is_classified() {
    let cli = callisto_cli::Cli::command();
    let mut found = BTreeSet::new();
    for name in ["release", "release-pr"] {
        let sub = cli.find_subcommand(name).expect("subcommand exists");
        collect(sub, name, &mut found);
    }
    let classified: BTreeSet<String> = FLAGS.iter().map(|(flag, ..)| (*flag).to_owned()).collect();
    assert_eq!(
        found, classified,
        "a path flag was added or removed: classify it in FLAGS (and give an output flag its tests)"
    );
}

fn run_dry(cwd: &Path, base: &[&str], flag: &str, value: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--cwd", cwd.to_str().unwrap(), "--dry-run"])
        .args(base)
        .args([flag, value])
        .output()
        .expect("callisto runs")
}

#[test]
fn output_flags_are_refused_under_dry_run_and_leave_an_existing_file_untouched() {
    for (name, role, base) in FLAGS {
        let Out(Refused(code)) = role else { continue };
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("existing.json");
        fs::write(&existing, "SENTINEL").unwrap();
        let flag = name.rsplit(' ').next().unwrap();
        let out = run_dry(dir.path(), base, flag, "existing.json");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{name} must be refused: {stderr}");
        assert!(stderr.contains(code), "{name} must fail with {code}: {stderr}");
        assert_eq!(fs::read_to_string(&existing).unwrap(), "SENTINEL", "{name}");
        let entries = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(entries, 1, "{name} must create nothing beside the existing file");
    }
}

/// Where a relative path must NOT resolve: the test process directory.
fn assert_not_in_process_dir(relative: &str) {
    let here = std::env::current_dir().unwrap().join(relative);
    assert!(!here.exists(), "{} must not be created", here.display());
}

const REL: &str = ".git/cwd-rule";

fn rel(name: &str) -> String {
    format!("{REL}/{name}")
}

fn prepare(root: &Path) {
    fs::create_dir_all(root.join(REL)).unwrap();
}

#[test]
fn release_plan_out_and_source_root_resolve_against_cwd() {
    let (dir, commit) = release_commit_fixture();
    let root = dir.path();
    prepare(root);
    let out = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--cwd", root.to_str().unwrap()])
        .args([
            "release",
            "plan",
            "--from-release-commit",
            &commit,
            "--decision",
            DECISION_PATH,
            "--out",
            &rel("intent.json"),
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(root.join(rel("intent.json")).is_file());
    assert_not_in_process_dir(REL);

    // `--source-root` relative to `--cwd`: run from the fixture's parent.
    let parent = root.parent().unwrap();
    let name = root.file_name().unwrap().to_str().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--cwd", parent.to_str().unwrap(), "--format", "json"])
        .args([
            "release",
            "plan",
            "--from-release-commit",
            &commit,
            "--decision",
            DECISION_PATH,
            "--source-root",
            name,
        ])
        .args(["--out", &format!("{name}/{}", rel("from-source-root.json"))])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(root.join(rel("from-source-root.json")).is_file());
}

#[test]
fn release_execute_intent_and_receipt_resolve_against_cwd() {
    let (dir, commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let root = dir.path();
    prepare(root);
    let absolute = plan_intent(root, external.path(), &commit);
    fs::copy(&absolute, root.join(rel("intent.json"))).unwrap();
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &commit, false);
    let publishers = FakePublishers {
        bin: &bin,
        log: &log,
        forge_marker: &forge_marker,
        git_trace: &git_trace,
    };
    let out = execute(
        root,
        Path::new(&rel("intent.json")),
        Path::new(&rel("receipt.json")),
        publishers,
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(root.join(rel("receipt.json")).is_file());
    assert_not_in_process_dir(REL);
}

#[test]
fn release_receipt_resolves_against_cwd_and_dry_run_ignores_it() {
    let (dir, commit) = release_commit_fixture();
    let root = dir.path();
    git(root, &["checkout", "-q", "main"]);
    prepare(root);
    let external = tempfile::tempdir().unwrap();
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &commit, false);
    fs::create_dir_all(external.path().join("home")).unwrap();
    let publishers = FakePublishers {
        bin: &bin,
        log: &log,
        forge_marker: &forge_marker,
        git_trace: &git_trace,
    };
    let home = external.path().join("home");

    let dry = release_local(
        root,
        &["--dry-run", "release", "--receipt", &rel("dry.json")],
        &home,
        publishers,
    );
    assert!(dry.status.success(), "{}", String::from_utf8_lossy(&dry.stderr));
    assert!(
        !root.join(rel("dry.json")).exists(),
        "--dry-run must not write the receipt"
    );

    let run = release_local(root, &["release", "--receipt", &rel("receipt.json")], &home, publishers);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    assert!(root.join(rel("receipt.json")).is_file());
    assert_not_in_process_dir(REL);
}

#[test]
fn release_artifact_manifest_flags_resolve_against_cwd() {
    let (dir, commit) = product_release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let root = dir.path();
    prepare(root);
    let intent = plan_product_intent(root, external.path(), &commit);
    fs::copy(&intent, root.join(rel("intent.json"))).unwrap();
    let artifacts = create_product_artifacts(external.path());
    let target = root.join(rel("artifacts"));
    fs::create_dir_all(&target).unwrap();
    for asset in PRODUCT_ASSETS {
        fs::copy(artifacts.join(asset), target.join(asset)).unwrap();
    }
    let out = callisto(
        root,
        &[
            "release",
            "artifact-manifest",
            "--intent",
            &rel("intent.json"),
            "--artifact-dir",
            &rel("artifacts"),
            "--out",
            &rel("manifest.json"),
        ],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(root.join(rel("manifest.json")).is_file());
    assert_not_in_process_dir(REL);
}

#[test]
fn release_inspect_input_resolves_against_cwd() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("doc.json"), "{\"a\":1}").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args([
            "--cwd",
            dir.path().to_str().unwrap(),
            "release",
            "inspect",
            "--input",
            "doc.json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn release_pr_commit_plan_out_resolves_against_cwd() {
    let dir = common::setup_polyglot_git_repo();
    let root = dir.path();
    prepare(root);
    let base = git(root, &["rev-parse", "HEAD"]);
    fs::write(root.join("NEW_FILE.txt"), "new\n").unwrap();
    git(root, &["add", "-A"]);
    let out = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--cwd", root.to_str().unwrap(), "release-pr", "commit-plan"])
        .args(["--base-commit", &base, "--message", "m", "--out", &rel("plan.json")])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(root.join(rel("plan.json")).is_file());
    assert_not_in_process_dir(REL);
}

#[test]
fn resolve_file_arg_joins_relative_paths_to_cwd_and_keeps_absolute() {
    use clap::Parser;
    let cli = callisto_cli::Cli::try_parse_from(["callisto", "--cwd", "/work", "status"]).unwrap();
    assert_eq!(cli.global.resolve_file_arg(Path::new("a/b")), Path::new("/work/a/b"));
    assert_eq!(cli.global.resolve_file_arg(Path::new("/abs/b")), Path::new("/abs/b"));
}
