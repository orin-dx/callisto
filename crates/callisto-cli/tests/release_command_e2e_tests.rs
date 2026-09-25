#![cfg(unix)]

//! SPEC-DX-RELEASE-COMMAND: bare `callisto release` and `--dry-run`, driven
//! through the compiled binary over a real Git repository with fake publishers.

#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;
use std::path::Path;

use release_harness::*;

struct Rig {
    external: tempfile::TempDir,
    bin: std::path::PathBuf,
    log: std::path::PathBuf,
    forge_marker: std::path::PathBuf,
    git_trace: std::path::PathBuf,
}

impl Rig {
    fn new(release_commit: &str) -> Self {
        Self::with_cargo_failure(release_commit, false)
    }

    fn with_cargo_failure(release_commit: &str, fail_cargo_publish: bool) -> Self {
        let external = tempfile::tempdir().unwrap();
        let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), release_commit, fail_cargo_publish);
        fs::create_dir_all(external.path().join("home")).unwrap();
        Rig {
            external,
            bin,
            log,
            forge_marker,
            git_trace,
        }
    }

    fn home(&self) -> std::path::PathBuf {
        self.external.path().join("home")
    }

    fn state(&self) -> std::path::PathBuf {
        self.external.path().join("state")
    }

    fn run(&self, root: &Path, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
        let state = self.state();
        let mut env = env.to_vec();
        env.push(("XDG_STATE_HOME", state.to_str().unwrap()));
        release_local(
            root,
            args,
            &self.home(),
            &env,
            FakePublishers {
                bin: &self.bin,
                log: &self.log,
                forge_marker: &self.forge_marker,
                git_trace: &self.git_trace,
            },
        )
    }

    fn effects(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

fn assert_no_effects(rig: &Rig, root: &Path) {
    let effects = rig.effects();
    for effect in ["cargo publish", "git push", "gh release create"] {
        assert!(!effects.contains(effect), "unexpected `{effect}`:\n{effects}");
    }
    assert!(git(root, &["tag", "--list", "core-crate@0.2.0"]).is_empty());
}

/// A merged release, checked out on its branch rather than detached.
fn on_branch() -> (tempfile::TempDir, String) {
    let (dir, release_commit) = release_commit_fixture();
    git(dir.path(), &["checkout", "-q", "main"]);
    (dir, release_commit)
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// AC-01, AC-04, AC-06, AC-09.
#[test]
fn release_on_a_branch_publishes_tags_and_releases_then_has_nothing_left() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);

    let preview = rig.run(root, &["--format", "json", "--dry-run", "release"], &[]);
    assert!(preview.status.success(), "{}", stderr(&preview));
    let planned: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_no_effects(&rig, root);

    let run = rig.run(root, &["--format", "json", "release"], &[]);
    assert!(run.status.success(), "{}\n{}", stderr(&run), rig.effects());
    let effects = rig.effects();
    for effect in ["cargo publish", "git push", "gh release create"] {
        assert_eq!(effects.matches(effect).count(), 1, "`{effect}` once:\n{effects}");
    }
    assert!(git(root, &["tag", "--list", "core-crate@0.2.0"]).contains("core-crate@0.2.0"));
    assert_eq!(git(root, &["symbolic-ref", "--short", "HEAD"]), "main");

    let receipt: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    assert_eq!(receipt["intentDigest"], planned["digest"], "dry-run plan is what ran");
    assert_eq!(
        receipt["envelope"]["orchestrationRevision"].as_str(),
        Some(release_commit.as_str())
    );
    assert_eq!(
        receipt["envelope"]["releaseSourceRevision"].as_str(),
        Some(release_commit.as_str())
    );

    let path = rig
        .state()
        .join("callisto")
        .read_dir()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join(planned["digest"].as_str().unwrap())
        .join("receipt.json");
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved, receipt);
    assert!(!path.starts_with(root));
    assert_eq!(git(root, &["status", "--porcelain"]), "", "no receipt in the checkout");

    let again = rig.run(root, &["--format", "text", "release"], &[]);
    assert!(again.status.success(), "{}", stderr(&again));
    assert_eq!(String::from_utf8_lossy(&again.stdout), "Nothing to release.\n");
    assert_eq!(rig.effects(), effects, "no-op performs no effect");
}

/// AC-09: `--receipt` overrides the state directory.
#[test]
fn receipt_flag_writes_to_the_given_path() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);
    let receipt = rig.external.path().join("explicit-receipt.json");

    let run = rig.run(root, &["release", "--receipt", receipt.to_str().unwrap()], &[]);
    assert!(run.status.success(), "{}", stderr(&run));
    assert!(receipt.is_file());
    assert!(!rig.state().exists());
    assert!(String::from_utf8_lossy(&run.stdout).contains(receipt.to_str().unwrap()));
}

/// AC-02, AC-08: a dirty worktree blocks the run but not the preview; ignored files never count.
#[test]
fn dirty_worktree_blocks_release_before_any_effect() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);

    fs::write(root.join("crates/core/src/lib.rs"), "pub fn changed() {}\n").unwrap();
    let tracked = rig.run(root, &["release"], &[]);
    assert!(!tracked.status.success());
    assert!(stderr(&tracked).contains("clean worktree"), "{}", stderr(&tracked));
    git(root, &["checkout", "--", "crates/core/src/lib.rs"]);

    fs::write(root.join("notes.txt"), "untracked\n").unwrap();
    let untracked = rig.run(root, &["release"], &[]);
    assert!(!untracked.status.success());
    assert!(stderr(&untracked).contains("clean worktree") && stderr(&untracked).contains("notes.txt"));

    let preview = rig.run(root, &["--format", "text", "--dry-run", "release"], &[]);
    assert!(preview.status.success(), "{}", stderr(&preview));
    let text = String::from_utf8_lossy(&preview.stdout);
    assert!(
        text.contains("cargo/core-crate 0.2.0") && text.contains("publish to cratesIo"),
        "{text}"
    );
    assert_no_effects(&rig, root);

    fs::remove_file(root.join("notes.txt")).unwrap();
    fs::write(root.join(".gitignore"), "scratch/\n").unwrap();
    git(root, &["add", ".gitignore"]);
    git(root, &["commit", "-q", "-m", "ignore scratch"]);
    fs::create_dir(root.join("scratch")).unwrap();
    fs::write(root.join("scratch/out"), "ignored\n").unwrap();
    let clean = rig.run(root, &["release"], &[]);
    assert!(clean.status.success(), "{}", stderr(&clean));
}

/// AC-03: `--package` restricts selection; a released or unknown package is refused.
#[test]
fn package_selection_is_exact() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);

    let selected = rig.run(
        root,
        &[
            "--format",
            "json",
            "--dry-run",
            "release",
            "--package",
            "cargo/core-crate",
        ],
        &[],
    );
    assert!(selected.status.success(), "{}", stderr(&selected));
    let intent: serde_json::Value = serde_json::from_slice(&selected.stdout).unwrap();
    assert_eq!(intent["decision"]["entries"][0]["package"], "cargo/core-crate");
    assert_eq!(
        intent["decision"]["entries"][0]["reasons"][0]["kind"],
        "unreleasedVersion"
    );

    let unknown = rig.run(root, &["--dry-run", "release", "--package", "cargo/nope"], &[]);
    assert!(!unknown.status.success());

    let commit = rig.run(root, &["release", "--from-release-commit", &release_commit], &[]);
    assert!(!commit.status.success());
    assert!(stderr(&commit).contains("--from-release-commit"), "{}", stderr(&commit));
}

/// AC-05: artifact slots require the CI route; the preview still works.
#[test]
fn artifact_slot_workspace_names_the_ci_route() {
    let (dir, release_commit) = product_release_commit_fixture();
    let root = dir.path();
    git(root, &["checkout", "-q", "main"]);
    let rig = Rig::new(&release_commit);

    let run = rig.run(root, &["--format", "json", "release"], &[]);
    assert!(!run.status.success());
    let err = stderr(&run);
    assert!(err.contains("release_requires_ci_route"), "{err}");
    for route in ["release plan", "release artifact-manifest", "release execute"] {
        assert!(err.contains(route), "missing `{route}`: {err}");
    }
    assert_no_effects(&rig, root);

    let preview = rig.run(root, &["--format", "json", "--dry-run", "release"], &[]);
    assert!(preview.status.success(), "{}", stderr(&preview));
    let intent: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(intent["artifactSlots"].as_array().unwrap().len(), 4);
}

/// AC-10: the legacy commands are gone from the binary.
#[test]
fn legacy_publish_commands_fail_to_parse() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);
    for command in ["publish", "plan-publish", "tag", "filter-plan"] {
        let out = rig.run(root, &[command], &[]);
        assert_eq!(out.status.code(), Some(2), "{command}");
        assert!(
            stderr(&out).contains("unrecognized subcommand"),
            "{command}: {}",
            stderr(&out)
        );
    }
}

/// Rewrites the release commit's committed decision to `schema_version` and returns the new commit.
fn release_commit_with_decision_schema(root: &Path, schema_version: u8) -> String {
    let path = root.join(DECISION_PATH);
    let mut decision: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    decision["schemaVersion"] = serde_json::json!(schema_version);
    fs::write(&path, serde_json::to_string_pretty(&decision).unwrap() + "\n").unwrap();
    git(root, &["add", DECISION_PATH]);
    git(root, &["commit", "-q", "--amend", "--no-edit"]);
    git(root, &["rev-parse", "HEAD"])
}

/// A release PR opened by an earlier build commits a schema-1 decision; it must still plan and execute.
#[test]
fn a_committed_v1_decision_still_plans_and_executes() {
    let (dir, _) = release_commit_fixture();
    let root = dir.path();
    let release_commit = release_commit_with_decision_schema(root, 1);
    let rig = Rig::new(&release_commit);

    let intent = plan_intent(root, rig.external.path(), &release_commit);
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&intent).unwrap()).unwrap();
    assert_eq!(value["decision"]["schemaVersion"], 2, "writers emit the current schema");

    let receipt = rig.external.path().join("receipt.json");
    let out = execute(
        root,
        &intent,
        &receipt,
        FakePublishers {
            bin: &rig.bin,
            log: &rig.log,
            forge_marker: &rig.forge_marker,
            git_trace: &rig.git_trace,
        },
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(receipt.is_file());
}

/// An unknown decision schema is refused and the error names the version found.
#[test]
fn an_unknown_decision_schema_is_rejected_by_version() {
    let (dir, _) = release_commit_fixture();
    let root = dir.path();
    let release_commit = release_commit_with_decision_schema(root, 3);
    let external = tempfile::tempdir().unwrap();
    let out = callisto(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            &release_commit,
            "--decision",
            DECISION_PATH,
            "--out",
            external.path().join("intent.json").to_str().unwrap(),
        ],
    );
    assert!(!out.status.success());
    assert!(stderr(&out).contains("schema version 3"), "{}", stderr(&out));
    assert!(!external.path().join("intent.json").exists());
}

/// Item 7: a preview without `origin` still prints the plan, with tags noted as unbound.
#[test]
fn dry_run_without_origin_notes_unbound_tags() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    git(root, &["remote", "remove", "origin"]);
    let rig = Rig::new(&release_commit);

    let preview = rig.run(root, &["--format", "text", "--dry-run", "release"], &[]);
    assert!(preview.status.success(), "{}", stderr(&preview));
    let text = String::from_utf8_lossy(&preview.stdout);
    assert!(
        text.contains("cargo/core-crate 0.2.0") && text.contains("create git tag"),
        "{text}"
    );
    assert_eq!(
        stderr(&preview).matches("tag operations are unbound").count(),
        1,
        "{}",
        stderr(&preview)
    );

    let run = rig.run(root, &["release"], &[]);
    assert!(!run.status.success());
    assert_no_effects(&rig, root);
}

/// L1: a mid-run failure writes no receipt; the rerun adopts what landed and writes one.
#[test]
fn a_failed_run_writes_no_receipt_and_a_rerun_completes() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let failing = Rig::with_cargo_failure(&release_commit, true);
    let out = failing.run(root, &["release"], &[]);
    assert!(!out.status.success());
    assert!(receipts(&failing.state()).is_empty(), "no receipt for a partial run");

    let rig = Rig::new(&release_commit);
    let rerun = rig.run(root, &["--format", "json", "release"], &[]);
    assert!(rerun.status.success(), "{}", stderr(&rerun));
    assert_eq!(receipts(&rig.state()).len(), 1);
}

fn receipts(dir: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .flat_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                receipts(&path)
            } else if path.file_name().is_some_and(|name| name == "receipt.json") {
                vec![path]
            } else {
                Vec::new()
            }
        })
        .collect()
}
