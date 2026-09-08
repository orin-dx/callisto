#![cfg(unix)]

//! Black-box durable-release acceptance tests.
//!
//! These tests intentionally drive the compiled `callisto` binary over a
//! real Git repository. Registry and forge programs are replaced only at the
//! process boundary, allowing the test to assert the observable release
//! contract without coupling to graph-private prepared-operation types.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use tempfile::TempDir;

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git").args(args).current_dir(root).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn callisto(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--format", "json", "--cwd", root.to_str().unwrap()])
        .args(args)
        .output()
        .expect("callisto binary should be invocable")
}

const DECISION_PATH: &str = ".callisto/release-decision.json";

fn system_git() -> PathBuf {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join("git"))
        .find(|candidate| {
            candidate.is_file()
                && Command::new(candidate)
                    .arg("--version")
                    .output()
                    .is_ok_and(|output| output.status.success())
        })
        .expect("a real Git executable must be available before installing the fixture PATH")
}

/// Builds the exact shape a merged release PR must have: its parent contains
/// a pending changeset, while its head changes the manifest and changelog and
/// removes that changeset.
fn release_commit_fixture() -> (TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Callisto Test"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["config", "tag.gpgsign", "false"]);
    git(
        root,
        &["remote", "add", "origin", "https://github.com/example/core-crate.git"],
    );

    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/core\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("crates/core/src")).unwrap();
    fs::write(
        root.join("crates/core/Cargo.toml"),
        "[package]\nname = \"core-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("crates/core/src/lib.rs"), "pub fn core() {}\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial workspace"]);

    let init = callisto(root, &["init", "--yes"]);
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    let config_path = root.join("callisto.toml");
    let config = fs::read_to_string(&config_path).unwrap();
    fs::write(
        config_path,
        format!(
            "{config}\n[[package]]\nmatch = \"cargo/core-crate\"\npublish-to = [\"crates-io\", \"github-release\"]\n"
        ),
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "configure callisto"]);

    let add = callisto(
        root,
        &[
            "add",
            "--package",
            "core-crate:minor",
            "--summary",
            "Ship durable release execution",
        ],
    );
    assert!(
        add.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "add release changeset"]);

    let version = callisto(root, &["version", "--emit-decision", DECISION_PATH]);
    assert!(
        version.status.success(),
        "version failed: {}",
        String::from_utf8_lossy(&version.stderr)
    );

    // `version` writes the reviewed manifest and changelog edits.  The release
    // PR, rather than an intermediate local command, consumes its exact
    // changeset when that PR is merged.  Model that merge boundary explicitly:
    // the parent contains the authority and the merge commit deletes it.
    for entry in fs::read_dir(root.join(".changeset")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|name| name != "README.md")
            && path.extension().is_some_and(|extension| extension == "md")
        {
            fs::remove_file(path).unwrap();
        }
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "release core-crate 0.2.0"]);
    let release_commit = git(root, &["rev-parse", "HEAD"]);
    // GitHub Actions checks out the merge commit detached.  Git-commit trust
    // deliberately requires that topology, rather than accepting a mutable
    // branch ref as release authority.
    git(root, &["checkout", "--detach", &release_commit]);
    (dir, release_commit)
}

/// Two packages in a `[[fixed-group]]`, one bumped by a changeset naming only
/// itself. This is the exact shape the prior re-derivation approach rejected:
/// its sibling's version changes with no changeset of its own naming it.
/// Both packages are seeded with a prior tag so the fixed-group target isn't
/// the `0.0.0` fallback a group with no prior release ever hits in practice.
fn fixed_group_release_commit_fixture() -> (TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Callisto Test"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["config", "tag.gpgsign", "false"]);
    git(
        root,
        &["remote", "add", "origin", "https://github.com/example/fixed-group.git"],
    );

    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/crate-a\", \"crates/crate-b\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    for name in ["crate-a", "crate-b"] {
        fs::create_dir_all(root.join(format!("crates/{name}/src"))).unwrap();
        fs::write(
            root.join(format!("crates/{name}/Cargo.toml")),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        fs::write(root.join(format!("crates/{name}/src/lib.rs")), "\n").unwrap();
    }
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial workspace"]);
    git(root, &["tag", "crate-a@0.1.0"]);
    git(root, &["tag", "crate-b@0.1.0"]);

    let init = callisto(root, &["init", "--yes"]);
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    let config_path = root.join("callisto.toml");
    let config = fs::read_to_string(&config_path).unwrap();
    fs::write(
        config_path,
        format!(
            "{config}\n\
             [[package]]\nmatch = \"cargo/crate-a\"\npublish-to = [\"crates-io\"]\n\n\
             [[package]]\nmatch = \"cargo/crate-b\"\npublish-to = [\"crates-io\"]\n\n\
             [[fixed-group]]\nname = \"demo\"\nmembers = [\"crate-a\", \"crate-b\"]\n"
        ),
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "configure callisto"]);

    let add = callisto(
        root,
        &["add", "--package", "crate-a:patch", "--summary", "Fix crate-a only"],
    );
    assert!(
        add.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "add release changeset"]);

    let version = callisto(root, &["version", "--emit-decision", DECISION_PATH]);
    assert!(
        version.status.success(),
        "version failed: {}",
        String::from_utf8_lossy(&version.stderr)
    );
    let crate_b_manifest = fs::read_to_string(root.join("crates/crate-b/Cargo.toml")).unwrap();
    assert!(
        crate_b_manifest.contains("0.1.1"),
        "crate-b must cascade to the fixed group's target version even though only \
         crate-a's changeset named it directly; manifest was:\n{crate_b_manifest}"
    );

    for entry in fs::read_dir(root.join(".changeset")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|name| name != "README.md")
            && path.extension().is_some_and(|extension| extension == "md")
        {
            fs::remove_file(path).unwrap();
        }
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "release fixed group 0.1.1"]);
    let release_commit = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "--detach", &release_commit]);
    (dir, release_commit)
}

/// The original bug this whole redesign exists to fix: a fixed-group cascade
/// bumps a sibling package with no changeset naming it directly. The prior
/// re-derivation approach only trusted a direct changeset-to-package match
/// and rejected exactly this commit shape in production.
#[test]
fn fixed_group_cascade_bump_without_direct_changeset_is_accepted() {
    let (dir, release_commit) = fixed_group_release_commit_fixture();
    let root = dir.path();
    let external = tempfile::tempdir().unwrap();
    let intent = external.path().join("release-intent.json");

    let plan = callisto(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            &release_commit,
            "--decision",
            DECISION_PATH,
            "--out",
            intent.to_str().unwrap(),
        ],
    );
    assert!(
        plan.status.success(),
        "release plan must accept a fixed-group cascade bump with no direct changeset: {}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&intent).unwrap()).unwrap();
    let packages = value["decision"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["package"].as_str().unwrap().to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        packages,
        std::collections::BTreeSet::from(["cargo/crate-a".to_owned(), "cargo/crate-b".to_owned()]),
        "both fixed-group members must be authorized, not just the one with a direct changeset"
    );
}

fn plan_intent(root: &Path, external: &Path, release_commit: &str) -> std::path::PathBuf {
    let intent = external.join("release-intent.json");
    let plan = callisto(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            release_commit,
            "--decision",
            DECISION_PATH,
            "--out",
            intent.to_str().unwrap(),
        ],
    );
    assert!(
        plan.status.success(),
        "release plan failed: {}\nrelease-commit delta:\n{}",
        String::from_utf8_lossy(&plan.stderr),
        git(
            root,
            &["diff-tree", "--no-commit-id", "--name-status", "-r", "HEAD^", "HEAD",],
        )
    );
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&intent).unwrap()).unwrap();
    assert_eq!(value["decision"]["entries"][0]["package"], "cargo/core-crate");
    assert_eq!(value["decision"]["entries"][0]["targetVersion"], "0.2.0");
    assert!(value["operations"]
        .as_array()
        .is_some_and(|operations| operations.len() >= 3));
    intent
}

fn fake_publishers(
    external: &Path,
    release_commit: &str,
    fail_cargo_publish: bool,
) -> (
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    use std::os::unix::fs::PermissionsExt;

    let bin = external.join("fake-bin");
    fs::create_dir_all(&bin).unwrap();
    let log = external.join("external-effects.log");
    let git_trace = external.join("git-commands.log");
    let forge_marker = external.join("forge-release-created");
    let cargo_exit = if fail_cargo_publish { "exit 23" } else { "exit 0" };
    fs::write(
        bin.join("cargo"),
        format!("#!/bin/sh\nprintf 'cargo %s\\n' \"$*\" >> \"$CALLISTO_TEST_LOG\"\n{cargo_exit}\n"),
    )
    .unwrap();
    fs::write(
        bin.join("git"),
        "#!/bin/sh\nprintf 'git %s\\n' \"$*\" >> \"$CALLISTO_TEST_GIT_TRACE\"\nif [ \"$1\" = push ]; then\n  printf 'git %s\\n' \"$*\" >> \"$CALLISTO_TEST_LOG\"\n  exit 0\nfi\nexec \"$CALLISTO_TEST_REAL_GIT\" \"$@\"\n",
    )
    .unwrap();
    fs::write(
        bin.join("gh"),
        format!(
            "#!/bin/sh\nprintf 'gh %s\\n' \"$*\" >> \"$CALLISTO_TEST_LOG\"\nif [ \"$1\" = api ]; then\n  if [ -f \"$CALLISTO_TEST_FORGE_MARKER\" ]; then\n    printf '%s\\n\\n%s\\n' 'HTTP/1.1 200 OK' '{{\"tag_name\":\"core-crate@0.2.0\",\"target_commitish\":\"{release_commit}\"}}'\n  else\n    printf '%s\\n\\n%s\\n' 'HTTP/1.1 404 Not Found' '{{}}'\n  fi\n  exit 0\nfi\nif [ \"$1\" = release ] && [ \"$2\" = create ]; then\n  : > \"$CALLISTO_TEST_FORGE_MARKER\"\nfi\nexit 0\n"
        ),
    )
    .unwrap();
    for program in [bin.join("cargo"), bin.join("git"), bin.join("gh")] {
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    }
    (bin, log, forge_marker, git_trace)
}

fn execute(
    root: &Path,
    intent: &Path,
    state: &Path,
    bin: &Path,
    log: &Path,
    forge_marker: &Path,
    git_trace: &Path,
) -> Output {
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--format", "json", "--cwd", root.to_str().unwrap()])
        .args([
            "release",
            "execute",
            "--intent",
            intent.to_str().unwrap(),
            "--state",
            state.to_str().unwrap(),
        ])
        .env("PATH", path)
        .env("CALLISTO_TEST_LOG", log)
        .env("CALLISTO_TEST_GIT_TRACE", git_trace)
        .env("CALLISTO_TEST_FORGE_MARKER", forge_marker)
        .env("CALLISTO_TEST_REAL_GIT", system_git())
        .output()
        .expect("release execute should run")
}

#[test]
fn merged_release_commit_executes_exactly_once_through_real_cli() {
    let (dir, release_commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let root = dir.path();
    let intent = plan_intent(root, external.path(), &release_commit);
    let state = external.path().join("release-state.json");
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, false);

    let first = execute(root, &intent, &state, &bin, &log, &forge_marker, &git_trace);
    assert!(
        first.status.success(),
        "release execute failed: {}\nGit command trace:\n{}\nEffect trace:\n{}",
        String::from_utf8_lossy(&first.stderr),
        fs::read_to_string(&git_trace).unwrap_or_else(|_| "<unavailable>".to_owned()),
        fs::read_to_string(&log).unwrap_or_else(|_| "<unavailable>".to_owned())
    );
    assert!(git(root, &["tag", "--list", "core-crate@0.2.0"]).contains("core-crate@0.2.0"));
    let effects = fs::read_to_string(&log).unwrap();
    assert!(effects.contains("cargo publish"));
    assert!(effects.contains("git push"));
    assert!(effects.contains("gh release create"));
    assert!(
        state.exists(),
        "durable execution state must be persisted outside implicit memory"
    );

    let second = execute(root, &intent, &state, &bin, &log, &forge_marker, &git_trace);
    assert!(
        second.status.success(),
        "a completed release must reconcile without retrying effects: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        effects,
        "a second execute must not republish, retag, or recreate the forge release"
    );
}

#[test]
fn failed_publish_persists_indeterminate_attempt_and_never_tags() {
    let (dir, release_commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let root = dir.path();
    let intent = plan_intent(root, external.path(), &release_commit);
    let state = external.path().join("release-state.json");
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, true);

    let output = execute(root, &intent, &state, &bin, &log, &forge_marker, &git_trace);
    assert!(
        !output.status.success(),
        "a failed registry publish must fail release execution"
    );
    assert!(fs::read_to_string(&log).unwrap().contains("cargo publish"));
    assert!(git(root, &["tag", "--list", "core-crate@0.2.0"]).is_empty());
    let state_json = fs::read_to_string(state).unwrap();
    assert!(
        state_json.contains("attempting"),
        "the executor must preserve an indeterminate attempt for reconciliation instead of inferring success"
    );
}

#[test]
fn changed_checkout_after_planning_never_reaches_a_publish_boundary() {
    let (dir, release_commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let root = dir.path();
    let intent = plan_intent(root, external.path(), &release_commit);
    let state = external.path().join("release-state.json");
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, false);

    // This simulates a checkout which changed after plan approval but before
    // the gated execution job.  It must fail before any publisher or forge
    // process is invoked; a stale intent is never a partial authorization.
    fs::write(
        root.join("crates/core/Cargo.toml"),
        "[package]\nname = \"core-crate\"\nversion = \"0.2.1\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let output = execute(root, &intent, &state, &bin, &log, &forge_marker, &git_trace);
    assert!(
        !output.status.success(),
        "a changed checkout must invalidate the approved release intent"
    );
    assert!(
        !log.exists(),
        "intent validation must fail before any external release side effect"
    );
    assert!(git(root, &["tag", "--list", "core-crate@0.2.0"]).is_empty());
    assert!(
        !state.exists(),
        "an invalid intent must not initialize execution state as though work began"
    );
}

/// A decision file that deserializes cleanly (its digest matches its own
/// entries) but whose claimed target version doesn't match what the commit
/// actually changed the manifest to. This is the diff-vs-decision cross-check,
/// distinct from digest tampering: nothing here is individually corrupt, the
/// two just disagree.
#[test]
fn release_plan_rejects_a_commit_whose_manifest_disagrees_with_its_own_decision() {
    let (dir, _release_commit) = release_commit_fixture();
    let root = dir.path();

    fs::write(
        root.join("crates/core/Cargo.toml"),
        "[package]\nname = \"core-crate\"\nversion = \"9.9.9\"\nedition = \"2021\"\n",
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "--amend", "--no-edit"]);
    let release_commit = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "--detach", &release_commit]);

    let external = tempfile::tempdir().unwrap();
    let out = external.path().join("must-not-exist.json");
    let result = callisto(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            &release_commit,
            "--decision",
            DECISION_PATH,
            "--out",
            out.to_str().unwrap(),
        ],
    );
    assert!(
        !result.status.success(),
        "a manifest that disagrees with the committed decision's claimed version must be rejected"
    );
    assert!(!out.exists());
}

/// A decision file whose entries were hand-edited after the fact no longer
/// matches its own content digest. `ReleaseDecisionV1`'s custom `Deserialize`
/// rejects this before the diff cross-check ever runs -- the digest is the
/// tamper-evidence primitive the whole persist-and-verify design rests on.
#[test]
fn release_plan_rejects_a_hand_tampered_decision_file() {
    let (dir, _release_commit) = release_commit_fixture();
    let root = dir.path();

    let decision_path = root.join(DECISION_PATH);
    let tampered = fs::read_to_string(&decision_path).unwrap().replace("0.2.0", "9.9.9");
    fs::write(&decision_path, tampered).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "--amend", "--no-edit"]);
    let release_commit = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "--detach", &release_commit]);

    let external = tempfile::tempdir().unwrap();
    let out = external.path().join("must-not-exist.json");
    let result = callisto(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            &release_commit,
            "--decision",
            DECISION_PATH,
            "--out",
            out.to_str().unwrap(),
        ],
    );
    assert!(
        !result.status.success(),
        "a decision file whose entries no longer match its own digest must be rejected"
    );
    assert!(!out.exists());
}

#[test]
fn release_plan_rejects_a_commit_that_is_not_checked_out_and_writes_nothing() {
    let (dir, release_commit) = release_commit_fixture();
    let root = dir.path();
    let parent = git(root, &["rev-parse", "HEAD^"]);
    let out = root.join("must-not-exist.json");
    let result = callisto(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            &parent,
            "--decision",
            DECISION_PATH,
            "--out",
            out.to_str().unwrap(),
        ],
    );
    assert!(!result.status.success());
    assert!(
        !out.exists(),
        "a stale release commit must not produce an intent that could later be executed; current release commit was {release_commit}"
    );
}
