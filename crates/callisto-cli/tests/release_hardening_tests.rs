#![cfg(unix)]

//! Black-box release hardening tests over the real `callisto` binary.
//!
//! Tests named `red_*` each pin a defect found in the release-lifecycle audit.

#[path = "common/release_harness.rs"]
mod release_harness;

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use release_harness::*;
use tempfile::TempDir;

fn execute_raw(
    root: &Path,
    intent: &Path,
    state: &Path,
    p: FakePublishers<'_>,
    forge_tag: &str,
    extra: &[&str],
    orchestration: Option<&str>,
) -> Output {
    let path = format!("{}:{}", p.bin.display(), std::env::var("PATH").unwrap());
    let head = git(root, &["rev-parse", "HEAD"]);
    let mut command = Command::new(env!("CARGO_BIN_EXE_callisto"));
    command
        .args(["--format", "json", "--cwd", root.to_str().unwrap()])
        .args([
            "release",
            "execute",
            "--intent",
            intent.to_str().unwrap(),
            "--state",
            state.to_str().unwrap(),
            "--receipt",
            state.with_extension("receipt.json").to_str().unwrap(),
            "--orchestration-revision",
            orchestration.unwrap_or(&head),
        ])
        .args(extra)
        .env("PATH", path)
        .env("CALLISTO_TEST_LOG", p.log)
        .env("CALLISTO_TEST_GIT_TRACE", p.git_trace)
        .env("CALLISTO_TEST_FORGE_MARKER", p.forge_marker)
        .env("CALLISTO_TEST_ARTIFACT_MARKER", p.log.with_extension("artifact-marker"))
        .env("CALLISTO_TEST_FORGE_TAG", forge_tag)
        .env("CALLISTO_TEST_CARGO_MARKER", registry_marker(root))
        .env("CALLISTO_TEST_REAL_GIT", system_git())
        .env("__CALLISTO_TEST_SLEEP_SCALE", "0");
    command.output().unwrap()
}

fn count(log: &Path, needle: &str) -> usize {
    fs::read_to_string(log).unwrap_or_default().matches(needle).count()
}

struct Env {
    _dir: TempDir,
    release_commit: String,
    external: TempDir,
    intent: PathBuf,
    state: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    forge_marker: PathBuf,
    git_trace: PathBuf,
}
impl Env {
    fn new(product: bool) -> Self {
        let (dir, release_commit) = if product {
            product_release_commit_fixture()
        } else {
            release_commit_fixture()
        };
        let external = tempfile::tempdir().unwrap();
        let intent = if product {
            plan_product_intent(dir.path(), external.path(), &release_commit)
        } else {
            plan_intent(dir.path(), external.path(), &release_commit)
        };
        let state = external.path().join("release-state.json");
        let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, false);
        Self {
            _dir: dir,
            release_commit,
            external,
            intent,
            state,
            bin,
            log,
            forge_marker,
            git_trace,
        }
    }
    fn root(&self) -> &Path {
        self._dir.path()
    }
    fn p(&self) -> FakePublishers<'_> {
        FakePublishers {
            bin: &self.bin,
            log: &self.log,
            forge_marker: &self.forge_marker,
            git_trace: &self.git_trace,
        }
    }
    fn run(&self, extra: &[&str]) -> Output {
        execute_raw(
            self.root(),
            &self.intent,
            &self.state,
            self.p(),
            "core-crate@0.2.0",
            extra,
            None,
        )
    }
    fn receipt(&self) -> serde_json::Value {
        serde_json::from_slice(&fs::read(self.state.with_extension("receipt.json")).unwrap()).unwrap()
    }
}
fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn p01_stranded_attempting_is_nonzero_and_repeats_no_effect() {
    let e = Env::new(false);
    // first run: publish fails -> Attempting is stranded
    drop(fake_publishers(e.external.path(), &e.release_commit, true));
    let first = e.run(&[]);
    assert!(!first.status.success());
    assert_eq!(count(&e.log, "cargo publish"), 1);
    // provider is now "healthy" but the effect's outcome is unknown
    drop(fake_publishers(e.external.path(), &e.release_commit, false));
    let second = e.run(&[]);
    assert!(
        !second.status.success(),
        "stranded Attempting must never resume into success: {}",
        stderr(&second)
    );
    assert_eq!(count(&e.log, "cargo publish"), 1, "no second publish");
    assert!(!e.state.with_extension("receipt.json").exists());
    assert!(fs::read_to_string(&e.state).unwrap().contains("attempting"));
    assert!(
        stderr(&second).contains("E173"),
        "typed unresolved-attempt diagnostic expected: {}",
        stderr(&second)
    );
}

#[test]
fn p02_partial_provider_success_recovers_from_fresh_runner_without_republishing() {
    let e = Env::new(false);
    // the registry already serves this version
    let marker = registry_marker(e.root());
    fs::write(marker.with_file_name("published.core-crate"), "0.2.0\n").unwrap();
    let out = e.run(&["--recovery"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(count(&e.log, "cargo publish"), 0);
    assert_eq!(count(&e.log, "gh release create"), 1);
    assert!(git(e.root(), &["tag", "--list", "core-crate@0.2.0"]).contains("core-crate@0.2.0"));
    let r = e.receipt();
    eprintln!("P02 receipt: {}", serde_json::to_string_pretty(&r).unwrap());
    assert_eq!(r["envelope"]["kind"], "recovery");
}

#[test]
fn p03_lost_state_without_recovery_flag_never_republishes() {
    let e = Env::new(false);
    assert!(e.run(&[]).status.success());
    fs::remove_file(&e.state).unwrap();
    fs::remove_file(e.state.with_extension("receipt.json")).unwrap();
    let before = count(&e.log, "cargo publish");
    let out = e.run(&[]);
    assert!(
        !out.status.success(),
        "normal run with lost state must not silently reconstruct: {}",
        stderr(&out)
    );
    assert_eq!(count(&e.log, "cargo publish"), before);
    assert!(!e.state.with_extension("receipt.json").exists());
}

#[test]
fn p04_tampered_profile_in_intent_is_rejected_before_any_effect() {
    let e = Env::new(false);
    let mut v: serde_json::Value = serde_json::from_slice(&fs::read(&e.intent).unwrap()).unwrap();
    v["profile"] = serde_json::Value::from("rehearsal");
    fs::write(&e.intent, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    let out = e.run(&["--profile", "rehearsal"]);
    assert!(!out.status.success(), "relabelled profile must break the intent digest");
    assert!(!e.log.exists() || count(&e.log, "cargo publish") == 0);
    assert!(!e.state.exists());
}

#[test]
fn p05_execute_profile_must_match_intent_profile() {
    let e = Env::new(false);
    let out = e.run(&["--profile", "rehearsal"]);
    assert!(!out.status.success());
    assert!(!e.log.exists());
}

#[test]
fn red_d05_malformed_orchestration_revision_is_rejected_before_any_effect() {
    let e = Env::new(false);
    let out = execute_raw(
        e.root(),
        &e.intent,
        &e.state,
        e.p(),
        "core-crate@0.2.0",
        &[],
        Some("not-a-sha"),
    );
    assert!(!out.status.success());
    assert_eq!(
        count(&e.log, "cargo publish"),
        0,
        "argument validation must precede release effects"
    );
    eprintln!(
        "P06 publish count after malformed orchestration revision: {}",
        count(&e.log, "cargo publish")
    );
}

#[test]
fn p07_plan_rejects_abbreviated_and_unknown_release_sha() {
    let (dir, release_commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    for bad in [&release_commit[..7], &"d".repeat(40)] {
        let out_path = external.path().join(format!("i-{}.json", bad.len()));
        let r = callisto(
            dir.path(),
            &[
                "release",
                "plan",
                "--from-release-commit",
                bad,
                "--decision",
                DECISION_PATH,
                "--out",
                out_path.to_str().unwrap(),
            ],
        );
        assert!(!r.status.success(), "{bad} must be rejected");
        assert!(!out_path.exists());
    }
}

#[test]
fn red_d06_unconfigured_profile_without_product_release_is_rejected() {
    let (dir, release_commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let out_path = external.path().join("i.json");
    let r = callisto(
        dir.path(),
        &[
            "release",
            "plan",
            "--profile",
            "does-not-exist",
            "--from-release-commit",
            &release_commit,
            "--decision",
            DECISION_PATH,
            "--out",
            out_path.to_str().unwrap(),
        ],
    );
    eprintln!(
        "P08 status={:?} intent_written={} stderr={}",
        r.status.code(),
        out_path.exists(),
        String::from_utf8_lossy(&r.stderr)
    );
    assert!(
        !r.status.success(),
        "unconfigured profile must fail before writing intent"
    );
    assert!(!out_path.exists());
}

#[test]
fn c4_source_without_release_section_plans_and_ignores_artifact_flags() {
    let (dir, release_commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let out_path = external.path().join("i.json");
    let r = callisto(
        dir.path(),
        &[
            "release",
            "plan",
            "--from-release-commit",
            &release_commit,
            "--decision",
            DECISION_PATH,
            "--orchestration-revision",
            &release_commit,
            "--artifact-repository",
            "example/repo",
            "--out",
            out_path.to_str().unwrap(),
        ],
    );
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    assert!(String::from_utf8_lossy(&r.stderr).contains("no [release] section"));
    let intent: serde_json::Value = serde_json::from_slice(&fs::read(&out_path).unwrap()).unwrap();
    assert!(intent["artifact_slots"].as_array().is_none_or(Vec::is_empty));
}

#[test]
fn p09_indeterminate_forge_observation_fails_closed() {
    let e = Env::new(false);
    assert!(e.run(&[]).status.success());
    fs::remove_file(&e.state).unwrap();
    fs::remove_file(e.state.with_extension("receipt.json")).unwrap();
    // gh now answers 401 for API reads
    fs::write(e.bin.join("gh"), unauthorized_gh()).unwrap();
    let before_create = count(&e.log, "gh release create");
    let out = e.run(&["--recovery"]);
    assert!(!out.status.success());
    assert_eq!(count(&e.log, "gh release create"), before_create);
    assert!(!e.state.with_extension("receipt.json").exists());
    assert!(
        stderr(&out).contains("E173"),
        "indeterminate observation must surface E173: {}",
        stderr(&out)
    );
}

#[test]
fn p10_conflicting_local_tag_fails_closed_and_issues_no_receipt() {
    let e = Env::new(false);
    git(
        e.root(),
        &["tag", "-a", "-m", "someone elses tag", "core-crate@0.2.0", "HEAD^"],
    );
    let out = e.run(&[]);
    assert!(!out.status.success());
    assert_eq!(count(&e.log, "gh release create"), 0);
    assert!(!e.state.with_extension("receipt.json").exists());
    eprintln!("P10 publish count = {}", count(&e.log, "cargo publish"));
}

#[test]
fn p11_receipt_is_bound_to_intent_and_provider_evidence() {
    let e = Env::new(false);
    assert!(e.run(&[]).status.success());
    let intent: serde_json::Value = serde_json::from_slice(&fs::read(&e.intent).unwrap()).unwrap();
    let r = e.receipt();
    eprintln!("P11 receipt: {}", serde_json::to_string_pretty(&r).unwrap());
    assert_eq!(r["envelope"]["intentDigest"], intent["digest"]);
    assert_eq!(r["envelope"]["profile"], "production");
    assert_eq!(r["envelope"]["kind"], "initial");
    let ops = intent["operations"].as_array().unwrap().len();
    assert_eq!(r["observations"].as_array().unwrap().len(), ops);
    assert!(r["observations"]
        .as_array()
        .unwrap()
        .iter()
        .all(|o| o["observation"]["kind"] == "exact"));
}

fn product_manifest(e: &Env) -> (PathBuf, PathBuf) {
    let artifacts = create_product_artifacts(e.external.path());
    let manifest = e.external.path().join("artifact-manifest.json");
    let m = callisto(
        e.root(),
        &[
            "release",
            "artifact-manifest",
            "--intent",
            e.intent.to_str().unwrap(),
            "--artifact-dir",
            artifacts.to_str().unwrap(),
            "--out",
            manifest.to_str().unwrap(),
        ],
    );
    assert!(m.status.success(), "{}", stderr(&m));
    (artifacts, manifest)
}

#[test]
fn p12_tampered_missing_or_swapped_artifacts_reject_before_any_effect() {
    for case in ["bytes", "missing", "manifest-digest"] {
        let e = Env::new(true);
        let (artifacts, manifest) = product_manifest(&e);
        match case {
            "bytes" => fs::write(artifacts.join("callisto-moon.wasm"), b"tampered").unwrap(),
            "missing" => fs::remove_file(artifacts.join("callisto-moon.wasm")).unwrap(),
            _ => {
                let mut m: serde_json::Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
                m["entries"][0]["digest"] = serde_json::Value::from("0".repeat(64));
                fs::write(&manifest, serde_json::to_vec(&m).unwrap()).unwrap();
            }
        }
        let out = execute_product(e.root(), &e.intent, &manifest, &artifacts, &e.state, e.p(), false);
        assert!(!out.status.success(), "{case}: must fail");
        assert_eq!(
            count(&e.log, "cargo publish"),
            0,
            "{case}: no effect before artifact verification"
        );
        assert_eq!(count(&e.log, "gh release"), 0, "{case}");
        eprintln!("P12 {case} stderr: {}", stderr(&out).lines().next().unwrap_or(""));
    }
}

#[test]
fn p13_concurrent_execute_publishes_at_most_once() {
    let e = Env::new(false);
    let spawn = |e: &Env| {
        let path = format!("{}:{}", e.bin.display(), std::env::var("PATH").unwrap());
        Command::new(env!("CARGO_BIN_EXE_callisto"))
            .args([
                "--format",
                "json",
                "--cwd",
                e.root().to_str().unwrap(),
                "release",
                "execute",
                "--intent",
                e.intent.to_str().unwrap(),
                "--state",
                e.state.to_str().unwrap(),
                "--receipt",
                e.state.with_extension("receipt.json").to_str().unwrap(),
                "--orchestration-revision",
                &git(e.root(), &["rev-parse", "HEAD"]),
            ])
            .env("PATH", path)
            .env("CALLISTO_TEST_LOG", &e.log)
            .env("CALLISTO_TEST_GIT_TRACE", &e.git_trace)
            .env("CALLISTO_TEST_FORGE_MARKER", &e.forge_marker)
            .env("CALLISTO_TEST_ARTIFACT_MARKER", e.log.with_extension("artifact-marker"))
            .env("CALLISTO_TEST_FORGE_TAG", "core-crate@0.2.0")
            .env("CALLISTO_TEST_CARGO_MARKER", registry_marker(e.root()))
            // slow publish so the two processes overlap
            .env("CALLISTO_TEST_CARGO_PUBLISH_SLEEP", "3")
            .env("CALLISTO_TEST_REAL_GIT", system_git())
            .env("__CALLISTO_TEST_SLEEP_SCALE", "0")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    };
    let a = spawn(&e);
    std::thread::sleep(std::time::Duration::from_millis(700));
    let b = spawn(&e);
    let (ao, bo) = (a.wait_with_output().unwrap(), b.wait_with_output().unwrap());
    eprintln!(
        "P13 a={:?} b={:?} b-stderr={}",
        ao.status.code(),
        bo.status.code(),
        String::from_utf8_lossy(&bo.stderr)
    );
    assert_eq!(count(&e.log, "cargo publish"), 1);
    assert!(ao.status.success() ^ bo.status.success() || (ao.status.success() && bo.status.success()));
}

#[test]
fn p14_recovery_with_a_conflicting_existing_tag_fails_closed() {
    let e = Env::new(false);
    assert!(e.run(&[]).status.success());
    fs::remove_file(&e.state).unwrap();
    fs::remove_file(e.state.with_extension("receipt.json")).unwrap();
    git(
        e.root(),
        &[
            "tag",
            "-a",
            "-f",
            "-m",
            "someone elses tag",
            "core-crate@0.2.0",
            "HEAD^",
        ],
    );
    let out = e.run(&["--recovery"]);
    assert!(
        !out.status.success(),
        "a tag pointing at another commit is a conflict, not an exact success"
    );
    assert!(stderr(&out).contains("E173"), "{}", stderr(&out));
    assert!(!e.state.with_extension("receipt.json").exists());
}

#[test]
fn p15_receipt_requires_fresh_provider_evidence_not_local_state() {
    let e = Env::new(false);
    assert!(e.run(&[]).status.success());
    fs::remove_file(e.state.with_extension("receipt.json")).unwrap();
    // local journal still says everything succeeded; the forge release vanishes
    fs::remove_file(&e.forge_marker).unwrap();
    let out = e.run(&[]);
    assert!(
        !out.status.success(),
        "receipt must not be issued from a stale local journal"
    );
    assert!(!e.state.with_extension("receipt.json").exists());
}

#[test]
fn p16_remote_asset_with_same_size_but_different_digest_is_a_conflict() {
    let e = Env::new(true);
    let (artifacts, manifest) = product_manifest(&e);
    let first = execute_product(e.root(), &e.intent, &manifest, &artifacts, &e.state, e.p(), false);
    assert!(first.status.success(), "{}", stderr(&first));
    fs::remove_file(&e.state).unwrap();
    fs::remove_file(e.state.with_extension("receipt.json")).unwrap();
    let marker = e.log.with_extension("artifact-marker");
    let tampered: Vec<String> = fs::read_to_string(&marker)
        .unwrap()
        .lines()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                let mut f: Vec<&str> = l.split('|').collect();
                let zero = "0".repeat(64);
                f[2] = &zero;
                f.join("|")
            } else {
                l.to_owned()
            }
        })
        .collect();
    fs::write(&marker, tampered.join("\n") + "\n").unwrap();
    let uploads = count(&e.log, "gh release upload");
    let out = execute_product(e.root(), &e.intent, &manifest, &artifacts, &e.state, e.p(), true);
    assert!(!out.status.success(), "a differing remote asset must not be adopted");
    assert!(stderr(&out).contains("E173"), "{}", stderr(&out));
    assert_eq!(count(&e.log, "gh release upload"), uploads);
    assert!(!e.state.with_extension("receipt.json").exists());
}

#[test]
fn p17_tampered_failed_state_reports_incomplete_release_e172() {
    let e = Env::new(false);
    drop(fake_publishers(e.external.path(), &e.release_commit, true));
    assert!(!e.run(&[]).status.success());
    drop(fake_publishers(e.external.path(), &e.release_commit, false));
    let tampered = fs::read_to_string(&e.state)
        .unwrap()
        .replace("\"attempting\"", "\"failed\"");
    fs::write(&e.state, tampered).unwrap();
    let out = e.run(&[]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("E172"), "{}", stderr(&out));
    assert!(!e.state.with_extension("receipt.json").exists());
    assert_eq!(count(&e.log, "cargo publish"), 1);
}

#[test]
fn p18_plan_rejects_artifact_repository_that_differs_from_the_git_remote() {
    let (dir, release_commit) = product_release_commit_fixture();
    git(
        dir.path(),
        &["remote", "set-url", "origin", "https://github.com/example/other.git"],
    );
    let external = tempfile::tempdir().unwrap();
    let intent = external.path().join("i.json");
    let r = callisto(
        dir.path(),
        &[
            "release",
            "plan",
            "--from-release-commit",
            &release_commit,
            "--decision",
            DECISION_PATH,
            "--orchestration-revision",
            &release_commit,
            "--artifact-repository",
            "example/core-crate",
            "--out",
            intent.to_str().unwrap(),
        ],
    );
    assert!(!r.status.success());
    assert!(stderr(&r).contains("E175"), "{}", stderr(&r));
    assert!(!intent.exists());
}

#[test]
fn p19_plan_rejects_artifact_repository_that_differs_from_profile_destination() {
    let (dir, release_commit) = product_release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let intent = external.path().join("i.json");
    let r = callisto(
        dir.path(),
        &[
            "release",
            "plan",
            "--from-release-commit",
            &release_commit,
            "--decision",
            DECISION_PATH,
            "--orchestration-revision",
            &release_commit,
            "--artifact-repository",
            "example/elsewhere",
            "--out",
            intent.to_str().unwrap(),
        ],
    );
    assert!(!r.status.success());
    assert!(stderr(&r).contains("targets forge repository"), "{}", stderr(&r));
    assert!(!intent.exists());
}

#[test]
fn red_p20_lightweight_preexisting_tag_reports_a_typed_conflict() {
    let e = Env::new(false);
    git(e.root(), &["tag", "core-crate@0.2.0", "HEAD^"]);
    let out = e.run(&[]);
    assert!(!out.status.success());
    eprintln!("P20 stderr: {}", stderr(&out));
    assert!(
        !stderr(&out).contains("E164"),
        "lightweight tag should be a typed conflict, not malformed-output: {}",
        stderr(&out)
    );
}

#[test]
fn red_d07_bare_product_package_is_rejected_or_still_yields_four_slots() {
    let (dir, release_commit) = product_release_commit_fixture();
    let cfg = dir.path().join("callisto.toml");
    let body = fs::read_to_string(&cfg).unwrap().replace(
        "product-package = \"cargo/core-crate\"",
        "product-package = \"core-crate\"",
    );
    fs::write(&cfg, body).unwrap();
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "--amend", "--no-edit"]);
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    git(dir.path(), &["checkout", "--detach", &head]);
    let external = tempfile::tempdir().unwrap();
    let intent = external.path().join("i.json");
    let r = callisto(
        dir.path(),
        &[
            "release",
            "plan",
            "--from-release-commit",
            &head,
            "--decision",
            DECISION_PATH,
            "--orchestration-revision",
            &head,
            "--artifact-repository",
            "example/core-crate",
            "--out",
            intent.to_str().unwrap(),
        ],
    );
    drop(release_commit);
    if r.status.success() {
        let v: serde_json::Value = serde_json::from_slice(&fs::read(&intent).unwrap()).unwrap();
        eprintln!(
            "P21 plan succeeded; artifactSlots={}",
            v["artifactSlots"].as_array().map_or(0, Vec::len)
        );
        assert_eq!(
            v["artifactSlots"].as_array().map(Vec::len),
            Some(4),
            "configured product assets silently dropped"
        );
    } else {
        eprintln!("P21 plan rejected: {}", stderr(&r));
    }
}

#[test]
fn p23_plan_and_execute_leave_source_and_coordinator_worktrees_clean() {
    let (source_dir, release_commit) = release_commit_fixture();
    let source = source_dir.path();
    let (coordinator_dir, _rev) = coordinator_checkout(source, &release_commit);
    let coordinator = coordinator_dir.path();
    let external = tempfile::tempdir().unwrap();
    let intent = plan_intent_from_source(coordinator, source, external.path(), &release_commit);
    assert_eq!(git(source, &["status", "--porcelain", "--untracked-files=all"]), "");
    assert_eq!(
        git(coordinator, &["status", "--porcelain", "--untracked-files=all"]),
        ""
    );
    let state = external.path().join("release-state.json");
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, false);
    let p = FakePublishers {
        bin: &bin,
        log: &log,
        forge_marker: &forge_marker,
        git_trace: &git_trace,
    };
    let out = execute_from_coordinator(coordinator, source, &intent, &state, p, false);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(git(source, &["status", "--porcelain", "--untracked-files=all"]), "");
    assert_eq!(
        git(coordinator, &["status", "--porcelain", "--untracked-files=all"]),
        ""
    );
}

// ---------------------------------------------------------------------
// Rig-based tests: fake providers that model the real tools.
// ---------------------------------------------------------------------

struct RigEnv {
    dir: TempDir,
    external: TempDir,
    intent: PathBuf,
    state: PathBuf,
    rig: Rig,
    forge_tag: &'static str,
}

impl RigEnv {
    fn from_fixture(
        dir: TempDir,
        release_commit: String,
        intent: impl FnOnce(&Path, &Path, &str) -> PathBuf,
        forge_tag: &'static str,
    ) -> Self {
        let external = tempfile::tempdir().unwrap();
        let intent = intent(dir.path(), external.path(), &release_commit);
        let state = external.path().join("release-state.json");
        let rig = Rig::new(external.path(), &release_commit);
        // Real repositories ignore `target/`; without this the fake cargo's build output would show as untracked.
        let exclude = dir.path().join(".git/info/exclude");
        let mut body = fs::read_to_string(&exclude).unwrap_or_default();
        body.push_str("target/\n");
        fs::write(exclude, body).unwrap();
        Self {
            dir,
            external,
            intent,
            state,
            rig,
            forge_tag,
        }
    }
    fn single() -> Self {
        let (dir, sha) = release_commit_fixture();
        Self::from_fixture(dir, sha, plan_intent, "core-crate@0.2.0")
    }
    fn product() -> Self {
        let (dir, sha) = product_release_commit_fixture();
        Self::from_fixture(dir, sha, plan_product_intent, "callisto@0.2.0")
    }
    fn fixed_group() -> Self {
        let (dir, sha) = fixed_group_release_commit_fixture();
        Self::from_fixture(dir, sha, plan_fixed_group_intent, "crate-a@0.1.1")
    }
    fn root(&self) -> &Path {
        self.dir.path()
    }
    fn run(&self, extra: &[&str]) -> Output {
        execute_rig(
            self.root(),
            &self.intent,
            &self.state,
            &self.rig,
            self.forge_tag,
            extra,
            None,
        )
    }
    fn receipt_path(&self) -> PathBuf {
        self.state.with_extension("receipt.json")
    }
    fn forget_state(&self) {
        fs::remove_file(&self.state).ok();
        fs::remove_file(self.receipt_path()).ok();
    }
    fn product_inputs(&self) -> (PathBuf, PathBuf) {
        let artifacts = create_product_artifacts(self.external.path());
        let manifest = self.external.path().join("artifact-manifest.json");
        let made = callisto(
            self.root(),
            &[
                "release",
                "artifact-manifest",
                "--intent",
                self.intent.to_str().unwrap(),
                "--artifact-dir",
                artifacts.to_str().unwrap(),
                "--out",
                manifest.to_str().unwrap(),
            ],
        );
        assert!(made.status.success(), "{}", stderr(&made));
        (artifacts, manifest)
    }
    fn run_product(&self, artifacts: &Path, manifest: &Path, recovery: bool) -> Output {
        let mut extra = vec![
            "--artifact-manifest",
            manifest.to_str().unwrap(),
            "--artifact-dir",
            artifacts.to_str().unwrap(),
        ];
        if recovery {
            extra.push("--recovery");
        }
        self.run(&extra)
    }
}

fn plan_fixed_group_intent(root: &Path, external: &Path, release_commit: &str) -> PathBuf {
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
    assert!(plan.status.success(), "fixed-group plan failed: {}", stderr(&plan));
    intent
}

/// The D01 defect was `cargo info` run without `--registry`: inside the
/// workspace it resolves the local manifest and reports the unpublished
/// version as already published. Observation is `cargo info` again -- so that
/// registry resolution and credentials are the package manager's job -- and the
/// property that makes it safe is asserted directly: every `cargo info` the
/// release path issues names a registry explicitly, and it is issued from the
/// workspace root where that registry is defined.
#[test]
fn red_d01_every_cargo_info_observation_names_a_registry_explicitly() {
    let e = RigEnv::single();
    let out = e.run(&[]);
    assert!(out.status.success(), "{}", stderr(&out));

    let calls = e.rig.cargo_calls();
    let info: Vec<&(String, String)> = calls.iter().filter(|(_, argv)| argv.starts_with("info ")).collect();
    assert!(
        !info.is_empty(),
        "the registry must be observed through cargo, got {calls:?}"
    );
    for (cwd, argv) in &info {
        assert!(
            argv.contains(" --registry "),
            "`cargo {argv}` has no --registry, so it would answer from the local manifest"
        );
        assert_eq!(
            std::fs::canonicalize(cwd).unwrap(),
            std::fs::canonicalize(e.root()).unwrap(),
            "observation must run from the workspace root whose config defines the registry"
        );
    }
    assert!(
        calls
            .iter()
            .all(|(_, argv)| argv.starts_with("info ") || argv.starts_with("publish ")),
        "only observation and the publish effect may shell to cargo, got {calls:?}"
    );
    assert_eq!(
        e.rig.log_count("cargo publish"),
        1,
        "an unpublished version must be published"
    );
}

#[test]
fn red_d02_release_runs_against_a_gh_that_rejects_unknown_api_flags() {
    let e = RigEnv::single();
    let out = e.run(&[]);
    assert!(
        out.status.success(),
        "strict gh rejected an emitted flag: {}\ngh calls: {:?}",
        stderr(&out),
        e.rig.gh_calls()
    );
    assert!(e.receipt_path().exists());
    assert_argv_within_allowlists(&e.rig.log, &e.rig.git_trace);
}

#[test]
fn red_d03_normal_run_succeeds_when_forge_reports_default_branch_as_target() {
    let mut e = RigEnv::product();
    e.rig.forge_default_branch("main");
    let (artifacts, manifest) = e.product_inputs();
    let out = e.run_product(&artifacts, &manifest, false);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(e.rig.log_count("gh release create"), 1);
    assert_eq!(
        e.rig.log_count("gh release upload"),
        4,
        "all four product assets must be uploaded"
    );
    assert!(e.receipt_path().exists());
}

#[test]
fn red_d03_recovery_run_succeeds_and_uploads_assets_when_forge_reports_default_branch() {
    let mut e = RigEnv::product();
    let (artifacts, manifest) = e.product_inputs();
    // Complete run against a forge that reports the SHA (the only shape the permissive fake can satisfy today).
    let first = e.run_product(&artifacts, &manifest, false);
    assert!(first.status.success(), "setup run failed: {}", stderr(&first));
    e.forget_state();
    // The forge release now exists but its assets are gone, and it reports the default branch.
    fs::remove_file(e.rig.log.with_extension("artifact-marker")).unwrap();
    e.rig.forge_default_branch("main");
    let uploads = e.rig.log_count("gh release upload");
    let out = e.run_product(&artifacts, &manifest, true);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        e.rig.log_count("gh release upload"),
        uploads + 4,
        "recovery must upload the missing assets"
    );
    assert!(e.receipt_path().exists());
}

#[test]
fn red_d04_two_crate_release_survives_cargo_leaving_a_target_dir() {
    let mut e = RigEnv::fixed_group();
    e.rig.real_cargo_target_dir();
    let out = e.run(&[]);
    assert!(
        out.status.success(),
        "release aborted after the first publish: {}\ndiagnostic codes: {:?}",
        stderr(&out),
        diagnostic_codes(&out)
    );
    assert_eq!(e.rig.log_count("cargo publish"), 2, "both crates must be published");
}

#[test]
fn real_cargo_package_creates_target_under_the_source_directory() {
    let Some(cargo) = std::env::var_os("CARGO") else {
        eprintln!("skipped: CARGO is not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Callisto Test"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    fs::write(root.join(".gitignore"), "target/\n").unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\nlicense = \"MIT\"\ndescription = \"x\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "tiny"]);
    let packaged = Command::new(cargo)
        .args(["package", "--offline", "--no-verify", "--allow-dirty"])
        .current_dir(root)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .output();
    let Ok(packaged) = packaged else {
        eprintln!("skipped: cargo could not be spawned");
        return;
    };
    assert!(
        packaged.status.success(),
        "{}",
        String::from_utf8_lossy(&packaged.stderr)
    );
    assert!(
        root.join("target/package").is_dir(),
        "real cargo must leave target/package under the source directory"
    );
}

#[test]
fn red_d08_local_only_tag_is_pushed_to_the_remote_before_the_receipt() {
    let mut e = RigEnv::single();
    let bare = bare_remote(e.external.path());
    e.rig.failing_git_push();
    let first = e.run(&[]);
    assert!(!first.status.success(), "setup: a failed push must fail the run");
    assert!(
        git(e.root(), &["tag", "--list", "core-crate@0.2.0"]).contains("core-crate@0.2.0"),
        "setup: the local tag must exist"
    );
    e.forget_state();
    e.rig.real_git_push(&bare, "https://github.com/example/core-crate.git");
    let second = e.run(&["--recovery"]);
    let remote_tags = git(&bare, &["tag", "--list", "core-crate@0.2.0"]);
    assert!(
        !e.receipt_path().exists() || remote_tags.contains("core-crate@0.2.0"),
        "a receipt was issued while the remote never received the tag"
    );
    assert!(second.status.success(), "{}", stderr(&second));
    assert!(
        remote_tags.contains("core-crate@0.2.0"),
        "the tag must reach the remote"
    );
}

#[test]
fn red_c7_pre_effect_observation_failure_does_not_wedge_rerun() {
    let e = RigEnv::single();
    // An unreachable registry proves neither presence nor absence, so after
    // the bounded retry it reaches the executor as an indeterminate answer.
    set_registry_unreachable(e.root(), true);
    let first = e.run(&[]);
    assert!(!first.status.success());
    assert_eq!(
        e.rig.log_count("cargo publish"),
        0,
        "no effect may run while observation fails"
    );
    set_registry_unreachable(e.root(), false);
    let second = e.run(&[]);
    assert!(
        second.status.success(),
        "rerun wedged; diagnostic codes {:?}: {}",
        diagnostic_codes(&second),
        stderr(&second)
    );
    assert_eq!(e.rig.log_count("cargo publish"), 1);
}

#[test]
fn c5_already_uploaded_text_without_registry_observation_is_not_success() {
    let mut e = RigEnv::single();
    e.rig.set("CALLISTO_TEST_CARGO_PUBLISH_EXIT", "101");
    e.rig.set(
        "CALLISTO_TEST_CARGO_PUBLISH_STDERR",
        "error: crate version `0.2.0` is already uploaded",
    );
    let out = e.run(&[]);
    eprintln!("C5 status={:?} codes={:?}", out.status.code(), diagnostic_codes(&out));
    assert!(
        !out.status.success() && !e.receipt_path().exists(),
        "the registry still does not serve the version, yet the run reported success (codes {:?})",
        diagnostic_codes(&out)
    );
}

#[test]
fn c5_already_exists_text_without_registry_observation_is_not_success() {
    let mut e = RigEnv::single();
    e.rig.set("CALLISTO_TEST_CARGO_PUBLISH_EXIT", "101");
    e.rig.set(
        "CALLISTO_TEST_CARGO_PUBLISH_STDERR",
        "error: failed to publish: crate version `0.2.0` already exists on crates.io index",
    );
    let out = e.run(&[]);
    eprintln!("C5 status={:?} codes={:?}", out.status.code(), diagnostic_codes(&out));
    assert!(
        !out.status.success() && !e.receipt_path().exists(),
        "the registry still does not serve the version, yet the run reported success (codes {:?})",
        diagnostic_codes(&out)
    );
}

/// F3: a sparse index that has not finished propagating must not cost a fully
/// published release its receipt.
///
/// The receipt pass observes every operation afresh. Before the fix it used
/// the plain observation, so a 404 served right after a successful publish was
/// read as "absent" and the receipt was refused -- after every crate, tag and
/// release had already been created. The rerun that follows a push release
/// carries no `--recovery`, so it could only end in E174.
#[test]
fn a_lagging_registry_after_publication_still_yields_a_receipt() {
    let e = RigEnv::single();
    // The post-publish confirmation gets its honest answer; the next two
    // reads -- the receipt pass -- answer 404, which it must treat as lag
    // rather than as an answer.
    set_registry_flap(e.root(), 1, 2);
    let out = e.run(&[]);
    assert!(
        out.status.success(),
        "index lag after a successful publish cost the release its receipt; codes {:?}: {}",
        diagnostic_codes(&out),
        stderr(&out)
    );
    assert!(
        e.receipt_path().exists(),
        "a successful run must leave the receipt it was asked for"
    );
    assert_eq!(
        e.rig.log_count("cargo publish"),
        1,
        "lag tolerance must not re-issue the publish"
    );
}

/// The other half of the same rule: tolerating lag must not turn a registry
/// that never serves the version into a success. A receipt still requires a
/// fresh exact observation of every operation.
#[test]
fn a_registry_that_never_serves_the_version_yields_no_receipt() {
    let e = RigEnv::single();
    // More 404s than the bounded retry policy will ever ask for, armed once
    // the publish itself has been confirmed.
    set_registry_flap(e.root(), 1, 1_000);
    let out = e.run(&[]);
    assert!(
        !out.status.success(),
        "a registry that never serves the version was reported as a successful release"
    );
    assert!(
        !e.receipt_path().exists(),
        "no receipt may be written when an operation was never observed exactly"
    );
}

/// `release plan` resolves its workspace with `find_workspace_root`; `execute`
/// used to canonicalise whatever `--cwd` named. Run from a subdirectory the
/// two then disagreed, and an intent rebuilt against a non-root directory
/// cannot match the one that was planned.
#[test]
fn execute_resolves_the_same_workspace_root_as_plan_from_a_subdirectory() {
    let e = RigEnv::single();
    let subdirectory = e.root().join("crates/core/src");
    assert!(subdirectory.is_dir(), "fixture must have a subdirectory to run from");
    let head = git(e.root(), &["rev-parse", "HEAD"]);
    let out = execute_rig_in(
        e.root(),
        &subdirectory,
        None,
        &e.intent,
        &e.state,
        &e.rig,
        e.forge_tag,
        &[],
        Some(&head),
    );
    assert!(
        out.status.success(),
        "execute from a subdirectory disagreed with plan's workspace root; codes {:?}: {}",
        diagnostic_codes(&out),
        stderr(&out)
    );
    assert!(e.receipt_path().exists(), "the subdirectory run must issue its receipt");
}

/// A bare `--state release-state.json` names a file in the process's own
/// working directory. Its directory is `Some("")`, which is not a directory
/// any later join can use meaningfully, so the state and its workspace lock
/// must still land there and the run must complete.
#[test]
fn a_bare_relative_state_path_resolves_against_the_process_working_directory() {
    let e = RigEnv::single();
    let elsewhere = tempfile::tempdir().unwrap();
    let head = git(e.root(), &["rev-parse", "HEAD"]);
    let out = execute_rig_in(
        e.root(),
        e.root(),
        Some(elsewhere.path()),
        &e.intent,
        Path::new("bare-release-state.json"),
        &e.rig,
        e.forge_tag,
        &[],
        Some(&head),
    );
    assert!(
        out.status.success(),
        "a bare --state path failed; codes {:?}: {}",
        diagnostic_codes(&out),
        stderr(&out)
    );
    assert!(
        elsewhere.path().join("bare-release-state.json").exists(),
        "the state file must land in the process working directory"
    );
    assert!(
        elsewhere.path().join("callisto/release-locks").is_dir(),
        "the workspace lock must live beside the state file it guards, not in an unrelated tree"
    );
}
