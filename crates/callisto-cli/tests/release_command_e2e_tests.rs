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

/// A first crates.io release of a crate with no description or license is refused before any effect.
#[test]
fn a_first_release_without_the_metadata_crates_io_requires_is_refused_before_any_effect() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);
    let manifest = root.join("crates/core/Cargo.toml");

    let complete = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        complete.replace("description = \"t\"\nlicense = \"MIT\"\n", ""),
    )
    .unwrap();
    let refused = rig.run(root, &["--dry-run", "release"]);
    assert!(!refused.status.success());
    let message = stderr(&refused);
    assert!(
        message.contains("E323") && message.contains("description or license"),
        "{message}"
    );
    assert_no_effects(&rig, root);

    fs::write(&manifest, complete).unwrap();
    let allowed = rig.run(root, &["--dry-run", "release"]);
    assert!(allowed.status.success(), "{}", stderr(&allowed));
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
    let decision: callisto_model::ReleaseDecisionV1 = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let mut decision = match schema_version {
        1 | 2 => serde_json::to_value(decision.older_schema(schema_version)).unwrap(),
        _ => serde_json::to_value(decision).unwrap(),
    };
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
    assert_eq!(
        value["decision"]["schemaVersion"], 1,
        "the decision keeps the schema it was committed with"
    );

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
    let release_commit = release_commit_with_decision_schema(root, 4);
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
    assert!(stderr(&out).contains("schema version 4"), "{}", stderr(&out));
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
    // The missing-origin probe must never leak git's own raw stderr for an absent remote -- only the one deliberate
    // note above.
    assert!(
        !stderr(&preview).contains("No such remote"),
        "git's own probe stderr must not leak: {}",
        stderr(&preview)
    );

    let run = rig.run(root, &["release"]);
    assert!(!run.status.success());
    assert_no_effects(&rig, root);
}

/// An `origin` that is not a hosted remote previews exactly like no `origin`, with or without a product release; a
/// real release refuses it by name.
#[test]
fn an_origin_that_is_not_hosted_previews_unbound_and_refuses_by_name() {
    let local = tempfile::tempdir().unwrap();
    let local = local.path().to_str().unwrap();
    for origin in [
        local,
        "file:///srv/git/core-crate.git",
        ".mirror:team/x",
        "remotes/origin.git",
    ] {
        for product_release in [false, true] {
            let (dir, release_commit) = release_commit_fixture_with_product_release(product_release);
            let root = dir.path();
            git(root, &["checkout", "-q", "main"]);
            git(root, &["remote", "set-url", "origin", origin]);
            let rig = Rig::new(&release_commit);

            let preview = rig.run(root, &["--format", "text", "--dry-run", "release"]);
            assert!(preview.status.success(), "{origin}: {}", stderr(&preview));
            let text = String::from_utf8_lossy(&preview.stdout);
            assert!(text.contains("cargo/core-crate 0.2.0"), "{origin}: {text}");
            assert_eq!(
                stderr(&preview).matches("not a hosted remote").count(),
                1,
                "{origin}: {}",
                stderr(&preview)
            );
            if product_release {
                continue;
            }
            let run = rig.run(root, &["release"]);
            assert!(!run.status.success());
            // The report wraps long values, so compare without its layout.
            let squash = |text: &str| {
                text.chars()
                    .filter(|c| !c.is_whitespace() && *c != '│')
                    .collect::<String>()
            };
            let refusal = squash(&stderr(&run));
            assert!(
                refusal.contains(&squash("`origin` is not a hosted remote")) && refusal.contains("git@host:owner/repo"),
                "{}",
                stderr(&run)
            );
            assert!(!refusal.contains(&squash(origin)), "{}", stderr(&run));
            assert!(
                refusal.contains(&squash("git remote get-url --push origin"))
                    && refusal.contains(&squash("git remote set-url --push origin <url>")),
                "{}",
                stderr(&run)
            );
            assert_no_effects(&rig, root);
        }
    }
}

/// A release with no `origin` says so and points at `git remote add`, not at a push URL that does not exist.
#[test]
fn a_release_without_an_origin_says_to_add_one() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    git(root, &["remote", "remove", "origin"]);
    let rig = Rig::new(&release_commit);
    let run = rig.run(root, &["release"]);
    assert!(!run.status.success());
    let refusal: String = stderr(&run)
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '│')
        .collect();
    assert!(refusal.contains("no`origin`remoteisconfigured"), "{}", stderr(&run));
    assert!(refusal.contains("gitremoteaddorigin<url>"), "{}", stderr(&run));
}

/// A selection that is not a release still gets the pending-work pointer before the error.
#[test]
fn a_selection_that_is_not_a_release_points_at_version_before_failing() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);
    let released = rig.run(root, &["release"]);
    assert!(released.status.success(), "{}", stderr(&released));
    let added = callisto(
        root,
        &["add", "--package", "cargo/core-crate:patch", "--summary", "Fix a thing"],
    );
    assert!(added.status.success(), "{}", stderr(&added));
    let run = rig.run(
        root,
        &[
            "--format",
            "text",
            "--dry-run",
            "release",
            "--package",
            "cargo/core-crate",
        ],
    );
    assert!(!run.status.success(), "{}", stderr(&run));
    assert!(stderr(&run).contains("E169"), "{}", stderr(&run));
    assert_eq!(
        stderr(&run).matches("run `callisto version`").count(),
        1,
        "{}",
        stderr(&run)
    );
}

/// A hosted `origin`, as an HTTPS URL or scp-style, binds the preview's tags.
#[test]
fn a_hosted_origin_binds_the_preview() {
    for origin in [
        "https://github.com/example/core-crate.git",
        "git@github.com:example/core-crate.git",
    ] {
        let (dir, release_commit) = on_branch();
        let root = dir.path();
        git(root, &["remote", "set-url", "origin", origin]);
        let rig = Rig::new(&release_commit);
        let preview = rig.run(root, &["--format", "text", "--dry-run", "release"]);
        assert!(preview.status.success(), "{origin}: {}", stderr(&preview));
        assert!(
            String::from_utf8_lossy(&preview.stdout).contains("create git tag"),
            "{origin}"
        );
        assert!(
            !stderr(&preview).contains("not a hosted remote"),
            "{origin}: {}",
            stderr(&preview)
        );
    }
}

/// A released version plus a new changeset has nothing to release yet, so the preview points at `callisto version`.
#[test]
fn nothing_to_release_with_a_new_changeset_points_at_version() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);
    let run = rig.run(root, &["release"]);
    assert!(run.status.success(), "{}", stderr(&run));
    let added = callisto(
        root,
        &["add", "--package", "cargo/core-crate:patch", "--summary", "Fix a thing"],
    );
    assert!(added.status.success(), "{}", stderr(&added));

    let preview = rig.run(root, &["--format", "text", "--dry-run", "release"]);
    assert!(preview.status.success(), "{}", stderr(&preview));
    assert_eq!(String::from_utf8_lossy(&preview.stdout), "Nothing to release.\n");
    assert_eq!(
        stderr(&preview).matches("callisto version").count(),
        1,
        "{}",
        stderr(&preview)
    );
}

/// A prerelease note `version` already applied stays on disk but is not pending, so the hint goes away.
#[test]
fn retained_prerelease_notes_are_not_pending() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);
    for args in [
        &["pre", "enter", "beta"][..],
        &["add", "--package", "cargo/core-crate:patch", "--summary", "Fix a thing"],
    ] {
        let out = callisto(root, args);
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
    }
    let hints = |rig: &Rig| {
        let preview = rig.run(root, &["--format", "text", "--dry-run", "release"]);
        assert!(preview.status.success(), "{}", stderr(&preview));
        stderr(&preview).matches("callisto version").count()
    };
    assert_eq!(hints(&rig), 1);

    let versioned = callisto(root, &["version"]);
    assert!(versioned.status.success(), "{}", stderr(&versioned));
    assert!(
        fs::read_dir(root.join(".changeset"))
            .unwrap()
            .flatten()
            .any(|entry| entry.path().extension().is_some_and(|ext| ext == "md")),
        "the note is retained for the prerelease cycle"
    );
    assert_eq!(hints(&rig), 0);
}

/// A preview plans what is already versioned, so pending changesets get a pointer to `callisto version`.
#[test]
fn dry_run_with_pending_changesets_points_at_version() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);
    let added = callisto(
        root,
        &["add", "--package", "cargo/core-crate:patch", "--summary", "Fix a thing"],
    );
    assert!(added.status.success(), "{}", stderr(&added));

    let preview = rig.run(root, &["--format", "text", "--dry-run", "release"]);
    assert!(preview.status.success(), "{}", stderr(&preview));
    assert_eq!(
        stderr(&preview).matches("callisto version").count(),
        1,
        "{}",
        stderr(&preview)
    );
}

/// `--package` narrows the pending-work hint to what the selection would release.
#[test]
fn the_pending_work_hint_follows_the_release_selection() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/core\", \"crates/other\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/other/src")).unwrap();
    fs::write(
        root.join("crates/other/Cargo.toml"),
        "[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2021\"\ndescription = \"t\"\nlicense = \"MIT\"\n",
    )
    .unwrap();
    fs::write(root.join("crates/other/src/lib.rs"), "").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "add other"]);
    let added = callisto(
        root,
        &["add", "--package", "cargo/other:patch", "--summary", "Fix other"],
    );
    assert!(added.status.success(), "{}", stderr(&added));
    let rig = Rig::new(&release_commit);

    let selected = rig.run(
        root,
        &[
            "--format",
            "text",
            "--dry-run",
            "release",
            "--package",
            "cargo/core-crate",
        ],
    );
    assert!(selected.status.success(), "{}", stderr(&selected));
    assert!(!stderr(&selected).contains("callisto version"), "{}", stderr(&selected));

    let everything = rig.run(root, &["--format", "text", "--dry-run", "release"]);
    assert!(everything.status.success(), "{}", stderr(&everything));
    assert!(
        stderr(&everything).contains("pending version work for `cargo/other`"),
        "{}",
        stderr(&everything)
    );
}

/// A one-crate repository with `origin` set to `origin`, committed.
fn init_fixture(origin: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    if let Some(origin) = origin {
        git(root, &["remote", "add", "origin", origin]);
    }
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"solo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "initial"]);
    dir
}

fn init_with(root: &Path, format: &str, env: &[(String, String)]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--format", format, "--cwd", root.to_str().unwrap(), "init", "--yes"])
        .args(["--versioning", "independent"])
        .envs(env.iter().map(|(key, value)| (key, value)))
        .output()
        .unwrap()
}

/// Each `init` note is said once on stderr, in text and JSON alike; JSON also carries it in the report.
fn assert_said_once(out: &std::process::Output, format: &str, note: &str) {
    assert!(out.status.success(), "{}", stderr(out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stderr(out).matches(note).count(), 1, "{format}: {}", stderr(out));
    if format == "json" {
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let messages = report["diagnostics"].as_array().unwrap();
        assert_eq!(
            messages
                .iter()
                .filter(|d| d["message"].as_str().unwrap().contains(note))
                .count(),
            1,
            "{report}"
        );
    } else {
        assert!(!stdout.contains(note), "{stdout}");
    }
}

#[test]
fn init_says_the_missing_origin_note_once_on_stderr() {
    for format in ["text", "json"] {
        let dir = init_fixture(None);
        let out = init_with(dir.path(), format, &[]);
        assert_said_once(&out, format, "No `origin` remote is configured");
        // The origin note covers it: no second, tags-unbound wording.
        assert!(!stderr(&out).contains("unbound"), "{format}: {}", stderr(&out));
    }
}

#[test]
fn init_says_a_non_hosted_origin_is_not_hosted_once_without_its_value() {
    for origin in [
        "/srv/vcs/team-repo.git",
        "file:///srv/vcs/team-repo.git",
        "https://u:secret@host:99999/x.git",
    ] {
        for format in ["text", "json"] {
            let dir = init_fixture(Some(origin));
            let out = init_with(dir.path(), format, &[]);
            assert_said_once(&out, format, "`origin` is not a hosted remote");
            let text = format!("{}{}", stderr(&out), String::from_utf8_lossy(&out.stdout));
            assert!(
                !text.contains("team-repo") && !text.contains("secret"),
                "{origin}: {text}"
            );
            assert!(!stderr(&out).contains("unbound"), "{origin}: {}", stderr(&out));
        }
    }
}

#[test]
fn init_says_why_it_skipped_an_unsupported_workflow_once_on_stderr() {
    for format in ["text", "json"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (path, content) in [
            (
                "package.json",
                r#"{"name":"root","version":"0.0.0","private":true,"workspaces":["web","web-linux-x64-gnu"]}"#,
            ),
            (
                "web/package.json",
                r#"{"name":"web","version":"1.0.0","optionalDependencies":{"web-linux-x64-gnu":"1.0.0"}}"#,
            ),
            (
                "web-linux-x64-gnu/package.json",
                r#"{"name":"web-linux-x64-gnu","version":"1.0.0","os":["linux"],"cpu":["x64"]}"#,
            ),
        ] {
            fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
            fs::write(root.join(path), content).unwrap();
        }
        callisto_fixtures::git::init_repo(root);
        git(
            root,
            &["remote", "add", "origin", "https://github.com/example/tools.git"],
        );
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "fixture"]);
        let out = init_with(root, format, &[]);
        assert_said_once(&out, format, "Skipping GitHub Actions workflow generation");
    }
}

#[test]
fn init_advises_protecting_the_release_branch_once_on_stderr() {
    // The workflow pins Callisto's own release tag: serve it from a local repository instead of GitHub.
    let upstream = tempfile::tempdir().unwrap();
    callisto_fixtures::git::init_repo(upstream.path());
    git(upstream.path(), &["commit", "-q", "--allow-empty", "-m", "release"]);
    git(
        upstream.path(),
        &["tag", &format!("callisto@{}", env!("CARGO_PKG_VERSION"))],
    );
    let env = [
        ("GIT_CONFIG_COUNT".to_owned(), "1".to_owned()),
        (
            "GIT_CONFIG_KEY_0".to_owned(),
            format!("url.file://{}.insteadOf", upstream.path().display()),
        ),
        (
            "GIT_CONFIG_VALUE_0".to_owned(),
            "https://github.com/orin-dx/callisto.git".to_owned(),
        ),
    ];
    for format in ["text", "json"] {
        let dir = init_fixture(Some("https://github.com/example/solo.git"));
        let out = init_with(dir.path(), format, &env);
        assert!(
            dir.path().join(".github/workflows/callisto-release.yml").is_file(),
            "{}",
            stderr(&out)
        );
        assert_said_once(&out, format, "require pull requests and reviews");
    }
}

/// A remote string, credentials included, is never echoed by init or release.
#[test]
fn a_credential_origin_never_reaches_any_output() {
    const ORIGIN: &str = "https://user:secret@host:99999/x.git";
    for format in ["text", "json"] {
        let dir = init_fixture(Some(ORIGIN));
        let out = init_with(dir.path(), format, &[]);
        assert!(out.status.success(), "{}", stderr(&out));
        for text in [stderr(&out), String::from_utf8_lossy(&out.stdout).into_owned()] {
            assert!(!text.contains("secret"), "{text}");
        }
    }

    let (dir, release_commit) = on_branch();
    let root = dir.path();
    git(root, &["remote", "set-url", "origin", ORIGIN]);
    let rig = Rig::new(&release_commit);
    for args in [&["--dry-run", "release"][..], &["release"]] {
        for format in ["text", "json"] {
            let out = rig.run(root, &[&["--format", format][..], args].concat());
            for text in [stderr(&out), String::from_utf8_lossy(&out.stdout).into_owned()] {
                assert!(!text.contains("secret"), "{args:?}: {text}");
            }
        }
    }
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

/// A tag made here that never reached `origin` is a release that may not have finished; `release` says so even when
/// it has nothing to plan, and stops saying so once the tag is pushed.
#[test]
fn a_tag_that_never_reached_origin_is_called_out() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);
    git(
        root,
        &[
            "tag",
            "-a",
            "-m",
            "Release core-crate@0.2.0",
            "core-crate@0.2.0",
            &release_commit,
        ],
    );

    let warned = rig.run(root, &["release", "--dry-run"]);
    assert!(
        stderr(&warned).contains("tag `core-crate@0.2.0` exists here but not on `origin`"),
        "{}",
        stderr(&warned)
    );

    // The remote now holds the tag, in the shape `ls-remote` reports an annotated one.
    let object = git(root, &["rev-parse", "refs/tags/core-crate@0.2.0"]);
    let remote_tags = format!("{}.remote-tags", rig.git_trace.display());
    fs::write(
        remote_tags,
        format!("{object}\trefs/tags/core-crate@0.2.0\n{release_commit}\trefs/tags/core-crate@0.2.0^{{}}\n"),
    )
    .unwrap();
    let quiet = rig.run(root, &["release", "--dry-run"]);
    assert!(!stderr(&quiet).contains("not on `origin`"), "{}", stderr(&quiet));
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

/// A first release whose version the registry holds for another repository is that project's, so it is refused.
#[test]
fn a_first_release_of_a_name_another_project_published_is_refused_before_any_effect() {
    let (dir, release_commit) = on_branch();
    let root = dir.path();
    let rig = Rig::new(&release_commit);
    fs::write(format!("{}.core-crate", registry_marker(root).display()), "0.2.0").unwrap();
    set_registry_repository(root, "https://github.com/someone-else/core-crate.git");

    for args in [&["release"][..], &["release", "--dry-run"][..]] {
        let refused = rig.run(root, args);
        assert!(!refused.status.success(), "{args:?}: {}", stderr(&refused));
        assert!(stderr(&refused).contains("E322"), "{args:?}: {}", stderr(&refused));
        assert_no_effects(&rig, root);
    }

    set_registry_repository(root, "git+https://GitHub.com/Example/core-crate.git");
    let same = rig.run(root, &["release", "--dry-run"]);
    assert!(same.status.success(), "{}", stderr(&same));
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
            format!("[package]\nname = \"dual\"\nversion = \"{version}\"\nedition = \"2021\"\ndescription = \"t\"\nlicense = \"MIT\"\n"),
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
