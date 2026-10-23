#![cfg(unix)]

//! Bare `callisto release` and `--dry-run`, driven
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

    fn run(&self, root: &Path, args: &[&str]) -> std::process::Output {
        release_local(
            root,
            args,
            &self.home(),
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

    /// The calls that change something; the log also records read-only observations.
    fn mutations(&self) -> Vec<String> {
        const MUTATING: [&str; 4] = ["cargo publish", "git push", "gh release create", "gh release edit"];
        self.effects()
            .lines()
            .filter(|line| MUTATING.iter().any(|prefix| line.starts_with(prefix)))
            .map(str::to_owned)
            .collect()
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

#[test]
fn release_on_a_branch_publishes_tags_and_releases_then_has_nothing_left() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);

    let preview = rig.run(root, &["--format", "json", "--dry-run", "release"]);
    assert!(preview.status.success(), "{}", stderr(&preview));
    let planned: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_no_effects(&rig, root);

    let run = rig.run(root, &["--format", "json", "release"]);
    assert!(run.status.success(), "{}\n{}", stderr(&run), rig.effects());
    let effects = rig.effects();
    for effect in ["cargo publish", "git push", "gh release create"] {
        assert_eq!(effects.matches(effect).count(), 1, "`{effect}` once:\n{effects}");
    }
    let mutations = rig.mutations();
    assert!(git(root, &["tag", "--list", "core-crate@0.2.0"]).contains("core-crate@0.2.0"));
    let annotation = git(root, &["tag", "-l", "--format=%(contents)", "core-crate@0.2.0"]);
    let hints = callisto_model::TagHints::parse(&annotation).expect("the release tag carries hints");
    assert_eq!(hints.package, "cargo/core-crate");
    assert_eq!(
        hints.trees["crates/core"],
        git(root, &["rev-parse", "core-crate@0.2.0^{commit}:crates/core"]),
        "the hint records the package directory's tree in the tagged commit"
    );
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

    assert_eq!(git(root, &["status", "--porcelain"]), "", "no receipt in the checkout");
    assert!(
        !rig.home().join(".local").exists() && !rig.home().join("Library").exists(),
        "no receipt without --receipt"
    );

    let again = rig.run(root, &["--format", "text", "release"]);
    assert!(again.status.success(), "{}", stderr(&again));
    assert_eq!(String::from_utf8_lossy(&again.stdout), "Nothing to release.\n");
    assert_eq!(rig.mutations(), mutations, "no-op performs no effect");
}

/// `--receipt` writes the receipt to that file.
#[test]
fn receipt_flag_writes_to_the_given_path() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);
    let receipt = rig.external.path().join("explicit-receipt.json");

    let run = rig.run(root, &["release", "--receipt", receipt.to_str().unwrap()]);
    assert!(run.status.success(), "{}", stderr(&run));
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
    assert_eq!(
        saved["envelope"]["releaseSourceRevision"].as_str(),
        Some(release_commit.as_str())
    );
    assert!(String::from_utf8_lossy(&run.stdout).contains(receipt.to_str().unwrap()));
}

/// A dirty worktree blocks the run but not the preview; ignored files never count.
#[test]
fn dirty_worktree_blocks_release_before_any_effect() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);

    fs::write(root.join("crates/core/src/lib.rs"), "pub fn changed() {}\n").unwrap();
    let tracked = rig.run(root, &["release"]);
    assert!(!tracked.status.success());
    assert!(stderr(&tracked).contains("clean worktree"), "{}", stderr(&tracked));
    git(root, &["checkout", "--", "crates/core/src/lib.rs"]);

    fs::write(root.join("notes.txt"), "untracked\n").unwrap();
    let untracked = rig.run(root, &["release"]);
    assert!(!untracked.status.success());
    assert!(stderr(&untracked).contains("clean worktree") && stderr(&untracked).contains("notes.txt"));

    let preview = rig.run(root, &["--format", "text", "--dry-run", "release"]);
    assert!(preview.status.success(), "{}", stderr(&preview));
    let text = String::from_utf8_lossy(&preview.stdout);
    assert!(
        text.contains("cargo/core-crate 0.2.0") && text.contains("publish to crates.io"),
        "{text}"
    );
    assert_no_effects(&rig, root);

    fs::remove_file(root.join("notes.txt")).unwrap();
    fs::write(root.join(".gitignore"), "scratch/\n").unwrap();
    git(root, &["add", ".gitignore"]);
    git(root, &["commit", "-q", "-m", "ignore scratch"]);
    fs::create_dir(root.join("scratch")).unwrap();
    fs::write(root.join("scratch/out"), "ignored\n").unwrap();
    let clean = rig.run(root, &["release"]);
    assert!(clean.status.success(), "{}", stderr(&clean));
}

/// `--package` restricts selection; a released or unknown package is refused.
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
    );
    assert!(selected.status.success(), "{}", stderr(&selected));
    let intent: serde_json::Value = serde_json::from_slice(&selected.stdout).unwrap();
    assert_eq!(intent["decision"]["entries"][0]["package"], "cargo/core-crate");
    assert_eq!(
        intent["decision"]["entries"][0]["reasons"][0]["kind"],
        "unreleasedVersion"
    );

    let unknown = rig.run(root, &["--dry-run", "release", "--package", "cargo/nope"]);
    assert!(!unknown.status.success());

    let commit = rig.run(root, &["release", "--from-release-commit", &release_commit]);
    assert!(!commit.status.success());
    assert!(stderr(&commit).contains("--from-release-commit"), "{}", stderr(&commit));
}

/// Artifact slots require the CI route; the preview still works.
#[test]
fn artifact_slot_workspace_names_the_ci_route() {
    let (dir, release_commit) = product_release_commit_fixture();
    let root = dir.path();
    git(root, &["checkout", "-q", "main"]);
    let rig = Rig::new(&release_commit);

    let run = rig.run(root, &["--format", "json", "release"]);
    assert!(!run.status.success());
    let err = stderr(&run);
    assert!(err.contains("E239"), "{err}");
    for route in ["release plan", "release artifact-manifest", "release execute"] {
        assert!(err.contains(route), "missing `{route}`: {err}");
    }
    assert_no_effects(&rig, root);

    let preview = rig.run(root, &["--format", "json", "--dry-run", "release"]);
    assert!(preview.status.success(), "{}", stderr(&preview));
    let intent: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(intent["artifactSlots"].as_array().unwrap().len(), 4);
}

/// The legacy commands are gone from the binary.
#[test]
fn legacy_publish_commands_fail_to_parse() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);
    for command in ["publish", "plan-publish", "tag", "filter-plan"] {
        let out = rig.run(root, &[command]);
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

    let preview = rig.run(root, &["--format", "text", "--dry-run", "release"]);
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
    // The missing-origin probe (`optional_git_remote`, called more than once
    // while deriving this preview) must never leak git's own raw stderr for
    // an absent remote -- only the one deliberate note above.
    assert!(
        !stderr(&preview).contains("No such remote"),
        "git's own probe stderr must not leak: {}",
        stderr(&preview)
    );

    let run = rig.run(root, &["release"]);
    assert!(!run.status.success());
    assert_no_effects(&rig, root);
}

/// L1: a mid-run failure writes no receipt; the rerun adopts what landed and writes one.
#[test]
fn a_failed_run_writes_no_receipt_and_a_rerun_completes() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let failing = Rig::with_cargo_failure(&release_commit, true);
    let receipt = failing.external.path().join("receipt.json");
    let out = failing.run(root, &["release", "--receipt", receipt.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(!receipt.exists(), "no receipt for a partial run");

    let rig = Rig::new(&release_commit);
    let rerun = rig.run(root, &["release", "--receipt", receipt.to_str().unwrap()]);
    assert!(rerun.status.success(), "{}", stderr(&rerun));
    assert!(receipt.is_file());
}

/// Makes every `git push` through the rig fail while `flag` exists.
fn fail_pushes_while(rig: &Rig, flag: &Path) {
    let git = rig.bin.join("git");
    let script = fs::read_to_string(&git).unwrap();
    let guard = format!(
        "#!/bin/sh\nif [ \"$1\" = push ] && [ -e '{}' ]; then printf 'fatal: unable to access remote\\n' >&2; exit 1; fi\n",
        flag.display()
    );
    fs::write(&git, script.replacen("#!/bin/sh\n", &guard, 1)).unwrap();
}

/// A local tag whose push failed must not read as a finished release: the rerun pushes it,
/// creates the GitHub release and writes the receipt.
#[test]
fn a_rerun_after_a_failed_tag_push_completes_the_release() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);
    let flag = rig.external.path().join("fail-pushes");
    fs::write(&flag, "").unwrap();
    fail_pushes_while(&rig, &flag);
    let receipt = rig.external.path().join("receipt.json");

    let failed = rig.run(root, &["release", "--receipt", receipt.to_str().unwrap()]);
    assert!(!failed.status.success(), "{}", stderr(&failed));
    assert!(!receipt.exists(), "no receipt for a partial run");
    assert!(git(root, &["tag", "--list", "core-crate@0.2.0"]).contains("core-crate@0.2.0"));

    fs::remove_file(&flag).unwrap();
    let rerun = rig.run(root, &["release", "--receipt", receipt.to_str().unwrap()]);
    assert!(rerun.status.success(), "{}", stderr(&rerun));
    assert_ne!(String::from_utf8_lossy(&rerun.stdout), "Nothing to release.\n");
    assert!(receipt.is_file(), "the rerun writes the receipt");
    let effects = rig.effects();
    assert_eq!(
        effects.matches("git push").count(),
        1,
        "the failed push is not logged:\n{effects}"
    );
    assert_eq!(effects.matches("gh release create").count(), 1, "{effects}");
    assert_eq!(
        effects.matches("cargo publish").count(),
        1,
        "the publish is adopted, not repeated:\n{effects}"
    );

    let again = rig.run(root, &["release"]);
    assert_eq!(String::from_utf8_lossy(&again.stdout), "Nothing to release.\n");
}

/// A release that stopped after publishing is finished from the commit that set the version: a later commit in the
/// package would be tagged with code the registry never received.
#[test]
fn a_published_version_is_not_tagged_on_a_later_commit() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);
    fs::write(format!("{}.core-crate", registry_marker(root).display()), "0.2.0").unwrap();
    fs::write(
        root.join("crates/core/src/lib.rs"),
        "pub fn core() {}\n// changed after the version\n",
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "change core after the version"]);

    let refused = rig.run(root, &["release"]);
    assert!(!refused.status.success(), "{}", stderr(&refused));
    assert!(stderr(&refused).contains("E320"), "{}", stderr(&refused));
    assert_no_effects(&rig, root);

    git(root, &["checkout", "--detach", "-q", &release_commit]);
    let finished = rig.run(root, &["release"]);
    assert!(finished.status.success(), "{}", stderr(&finished));
    assert_eq!(git(root, &["rev-list", "-n", "1", "core-crate@0.2.0"]), release_commit);
    assert_eq!(
        rig.effects().matches("cargo publish").count(),
        0,
        "adopted, not published again"
    );
}

/// The preview of an interrupted release shows the remaining work, and a selected package whose
/// release is complete is still not a release.
#[test]
fn a_dry_run_after_a_failed_tag_push_plans_the_remaining_operations() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);
    let flag = rig.external.path().join("fail-pushes");
    fs::write(&flag, "").unwrap();
    fail_pushes_while(&rig, &flag);
    assert!(!rig.run(root, &["release"]).status.success());
    fs::remove_file(&flag).unwrap();

    let preview = rig.run(root, &["--format", "json", "--dry-run", "release"]);
    assert!(preview.status.success(), "{}", stderr(&preview));
    let planned: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert!(planned.get("nothingToRelease").is_none(), "{planned}");

    let run = rig.run(root, &["release", "--package", "cargo/core-crate"]);
    assert!(run.status.success(), "{}", stderr(&run));
    let selected_again = rig.run(root, &["release", "--package", "cargo/core-crate"]);
    assert!(!selected_again.status.success(), "a finished release is not selectable");
}

/// A release settled on an older commit is never observed: no provider is contacted.
#[test]
fn a_release_settled_on_an_older_commit_reaches_no_provider() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);
    assert!(rig.run(root, &["release"]).status.success());
    fs::write(root.join("crates/core/src/lib.rs"), "pub fn later() {}\n").unwrap();
    git(root, &["commit", "-qam", "later work"]);
    let before = rig.effects();

    let out = rig.run(root, &["release"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Nothing to release.\n");
    assert_eq!(rig.effects(), before, "no provider was observed");
}

/// With nothing left to release, machine consumers get a typed envelope, not prose, from both
/// the preview and the real run, and neither performs an effect.
#[test]
fn nothing_to_release_is_a_typed_envelope_in_json_for_preview_and_run() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);
    let first = rig.run(root, &["--format", "json", "release"]);
    assert!(first.status.success(), "{}", stderr(&first));
    let mutations = rig.mutations();

    for (args, dry_run) in [
        (&["--format", "json", "--dry-run", "release"][..], true),
        (&["--format", "json", "release"][..], false),
    ] {
        let out = rig.run(root, args);
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["nothingToRelease"], true, "{args:?}: {report}");
        assert_eq!(report["dryRun"], dry_run, "{args:?}: {report}");
    }
    assert_eq!(rig.mutations(), mutations, "reporting nothing performs no effect");
}

/// A Cargo and npm package in one directory, released as `dual@1.0.0` and now at 1.1.0.
fn unreleased_dual_package() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    git(
        root,
        &["remote", "add", "origin", "https://github.com/example/dual.git"],
    );
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"dual\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("dual/src")).unwrap();
    fs::write(root.join("dual/src/lib.rs"), "\n").unwrap();
    let write_versions = |version: &str| {
        fs::write(
            root.join("dual/Cargo.toml"),
            format!("[package]\nname = \"dual\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        fs::write(
            root.join("dual/package.json"),
            format!("{{\n  \"name\": \"dual\",\n  \"version\": \"{version}\"\n}}\n"),
        )
        .unwrap();
    };
    callisto_fixtures::scaffold_callisto(root);
    write_versions("1.0.0");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "release dual 1.0.0"]);
    git(root, &["tag", "dual@1.0.0"]);
    write_versions("1.1.0");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "bump dual to 1.1.0"]);
    dir
}

#[test]
fn selecting_one_distribution_of_a_dual_package_releases_both() {
    let dir = unreleased_dual_package();
    let root = dir.path();
    for selection in ["npm/dual", "cargo/dual"] {
        let out = callisto(root, &["--dry-run", "release", "--package", selection]);
        assert!(
            out.status.success(),
            "{selection}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let plan: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let mut publishes: Vec<&str> = plan["operations"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|operation| operation["id"]["role"]["kind"] == "registryPublish")
            .filter_map(|operation| operation["id"]["package"].as_str())
            .collect();
        publishes.sort_unstable();
        assert_eq!(publishes, ["cargo/dual", "npm/dual"], "{selection}: {plan}");
    }
}
