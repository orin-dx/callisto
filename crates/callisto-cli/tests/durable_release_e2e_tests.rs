#![cfg(unix)]

//! Black-box durable-release acceptance tests.
//!
//! These tests intentionally drive the compiled `callisto` binary over a
//! real Git repository. Registry and forge programs are replaced only at the
//! process boundary, allowing the test to assert the observable release
//! contract without coupling to graph-private prepared-operation types.

#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;

use release_harness::*;

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
#[test]
fn product_release_rejects_an_unconfigured_profile_before_writing_intent() {
    let (dir, release_commit) = product_release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let intent = external.path().join("rehearsal-intent.json");
    let plan = callisto(
        dir.path(),
        &[
            "release",
            "plan",
            "--profile",
            "rehearsal",
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
    assert!(
        !plan.status.success(),
        "an unconfigured rehearsal destination must fail before creating an intent"
    );
    assert!(
        String::from_utf8_lossy(&plan.stderr).contains("release profile `rehearsal` is not configured"),
        "profile failure should say why provisioning is required: {}",
        String::from_utf8_lossy(&plan.stderr)
    );
    assert!(!intent.exists(), "failed profile validation must not write an intent");
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
    assert_argv_within_allowlists(&log, &git_trace);
    assert!(effects.contains("git push"));
    assert!(effects.contains("gh release create"));
    assert!(
        state.exists(),
        "durable execution state must be persisted outside implicit memory"
    );
    assert!(
        state.with_extension("receipt.json").exists(),
        "a successful release must persist a provider-observed terminal receipt"
    );

    let second = execute(root, &intent, &state, &bin, &log, &forge_marker, &git_trace);
    assert!(
        second.status.success(),
        "a completed release must reconcile without retrying effects: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let after_second_execute = fs::read_to_string(&log).unwrap();
    for effect in ["cargo publish", "git push", "gh release create"] {
        assert_eq!(
            after_second_execute.matches(effect).count(),
            effects.matches(effect).count(),
            "a second execute may re-observe providers for its receipt, but must not repeat `{effect}`"
        );
    }
}

#[test]
fn product_artifacts_are_uploaded_once_and_recovered_from_provider_observation() {
    let (dir, release_commit) = product_release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let root = dir.path();
    let intent = plan_product_intent(root, external.path(), &release_commit);
    let artifacts = create_product_artifacts(external.path());
    let manifest = external.path().join("artifact-manifest.json");
    let create_manifest = callisto(
        root,
        &[
            "release",
            "artifact-manifest",
            "--intent",
            intent.to_str().unwrap(),
            "--artifact-dir",
            artifacts.to_str().unwrap(),
            "--out",
            manifest.to_str().unwrap(),
        ],
    );
    assert!(
        create_manifest.status.success(),
        "artifact manifest creation failed: {}",
        String::from_utf8_lossy(&create_manifest.stderr)
    );
    let state = external.path().join("release-state.json");
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, false);
    let publishers = FakePublishers {
        bin: &bin,
        log: &log,
        forge_marker: &forge_marker,
        git_trace: &git_trace,
    };

    let first = execute_product(root, &intent, &manifest, &artifacts, &state, publishers, false);
    assert!(
        first.status.success(),
        "product release execution failed: {}\n{}",
        String::from_utf8_lossy(&first.stderr),
        fs::read_to_string(&log).unwrap_or_else(|_| "<no effect trace>".to_owned()),
    );
    let effects = fs::read_to_string(&log).unwrap();
    for asset in PRODUCT_ASSETS {
        assert!(
            effects.contains("gh release upload callisto@0.2.0") && effects.contains(asset),
            "each planned product artifact must be uploaded: missing {asset} in {effects}"
        );
    }
    assert_eq!(effects.matches("gh release upload").count(), PRODUCT_ASSETS.len());
    assert!(!effects.contains(".hidden-metadata"));
    assert!(!effects.contains("nested/unlisted-artifact"));

    fs::remove_file(&state).unwrap();
    fs::remove_file(state.with_extension("receipt.json")).unwrap();
    let recovered = execute_product(root, &intent, &manifest, &artifacts, &state, publishers, true);
    assert!(
        recovered.status.success(),
        "fresh-state product recovery failed: {}\n{}",
        String::from_utf8_lossy(&recovered.stderr),
        fs::read_to_string(&log).unwrap_or_else(|_| "<no effect trace>".to_owned()),
    );
    let after_recovery = fs::read_to_string(&log).unwrap();
    assert_eq!(
        after_recovery.matches("gh release upload").count(),
        PRODUCT_ASSETS.len(),
        "provider-observed recovery must never upload an existing product artifact again"
    );
    assert!(state.with_extension("receipt.json").exists());
    assert_argv_within_allowlists(&log, &git_trace);
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
fn explicit_recovery_reconstructs_missing_state_from_remote_evidence() {
    let (dir, release_commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let root = dir.path();
    let intent = plan_intent(root, external.path(), &release_commit);
    let state = external.path().join("release-state.json");
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, false);

    let first = execute(root, &intent, &state, &bin, &log, &forge_marker, &git_trace);
    assert!(
        first.status.success(),
        "initial execution must establish remote state: {}\n{}\n{}",
        String::from_utf8_lossy(&first.stderr),
        fs::read_to_string(&git_trace).unwrap_or_else(|_| "<no git trace>".to_owned()),
        fs::read_to_string(&log).unwrap_or_else(|_| "<no effect trace>".to_owned()),
    );
    fs::remove_file(&state).unwrap();
    fs::remove_file(state.with_extension("receipt.json")).unwrap();
    let effects = fs::read_to_string(&log).unwrap();

    let recovered = execute_with_recovery(
        root,
        &intent,
        &state,
        FakePublishers {
            bin: &bin,
            log: &log,
            forge_marker: &forge_marker,
            git_trace: &git_trace,
        },
        true,
    );
    assert!(
        recovered.status.success(),
        "explicit recovery must reconstruct a missing local journal from exact provider state: {}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    let after_recovery = fs::read_to_string(&log).unwrap();
    for effect in ["cargo publish", "git push", "gh release create"] {
        assert_eq!(
            after_recovery.matches(effect).count(),
            effects.matches(effect).count(),
            "recovery must not repeat `{effect}` after provider reconstruction"
        );
    }
    assert!(state.with_extension("receipt.json").exists());
    assert_argv_within_allowlists(&log, &git_trace);
}

#[test]
fn newer_coordinator_executes_and_recovers_an_older_release_source() {
    let (source_dir, release_commit) = release_commit_fixture();
    let source = source_dir.path();
    let (coordinator_dir, coordinator_revision) = coordinator_checkout(source, &release_commit);
    let coordinator = coordinator_dir.path();
    let external = tempfile::tempdir().unwrap();
    let intent = plan_intent_from_source(coordinator, source, external.path(), &release_commit);
    let state = external.path().join("release-state.json");
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, false);
    let publishers = FakePublishers {
        bin: &bin,
        log: &log,
        forge_marker: &forge_marker,
        git_trace: &git_trace,
    };

    let first = execute_from_coordinator(coordinator, source, &intent, &state, publishers, false);
    assert!(
        first.status.success(),
        "new coordinator execution failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(state.with_extension("receipt.json")).expect("initial receipt must exist"))
            .unwrap();
    assert_eq!(receipt["envelope"]["orchestrationRevision"], coordinator_revision);
    assert_eq!(receipt["envelope"]["releaseSourceRevision"], release_commit);

    fs::remove_file(&state).unwrap();
    fs::remove_file(state.with_extension("receipt.json")).unwrap();
    let effects = fs::read_to_string(&log).unwrap();
    let recovered = execute_from_coordinator(coordinator, source, &intent, &state, publishers, true);
    assert!(
        recovered.status.success(),
        "new coordinator recovery failed: {}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    let after_recovery = fs::read_to_string(&log).unwrap();
    for effect in ["cargo publish", "git push", "gh release create"] {
        assert_eq!(
            after_recovery.matches(effect).count(),
            effects.matches(effect).count(),
            "recovery must observe the old source's remote effects instead of repeating `{effect}`"
        );
    }
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
