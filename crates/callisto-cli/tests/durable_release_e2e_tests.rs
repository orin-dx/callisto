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
fn execute_rejects_an_intent_whose_artifact_repository_is_not_the_configured_one() {
    let (dir, release_commit) = product_release_commit_fixture();
    let root = dir.path();
    let external = tempfile::tempdir().unwrap();
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
    assert!(create_manifest.status.success());
    let config = root.join("callisto.toml");
    let moved = fs::read_to_string(&config).unwrap().replace(
        "forge-repository = \"example/core-crate\"",
        "forge-repository = \"example/moved\"",
    );
    fs::write(&config, moved).unwrap();
    let receipt = external.path().join("release-receipt.json");
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, false);
    let out = execute_product(
        root,
        &intent,
        &manifest,
        &artifacts,
        &receipt,
        FakePublishers {
            bin: &bin,
            log: &log,
            forge_marker: &forge_marker,
            git_trace: &git_trace,
        },
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        err.contains("E230")
            && err.contains("`example/moved`")
            && err.contains("`example/core-crate`")
            && !err.contains("profile"),
        "{err}"
    );
    assert!(!log.exists() || !fs::read_to_string(&log).unwrap().contains("cargo publish"));
    assert!(!receipt.exists());
}

#[test]
fn merged_release_commit_executes_exactly_once_through_real_cli() {
    let (dir, release_commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let root = dir.path();
    let intent = plan_intent(root, external.path(), &release_commit);
    let receipt = external.path().join("release-receipt.json");
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, false);

    let publishers = FakePublishers {
        bin: &bin,
        log: &log,
        forge_marker: &forge_marker,
        git_trace: &git_trace,
    };

    let first = execute(root, &intent, &receipt, publishers);
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
    assert!(receipt.exists(), "a successful release must persist a terminal receipt");

    let second = execute(root, &intent, &receipt, publishers);
    assert!(
        second.status.success(),
        "a completed release must be adopted without retrying effects: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let after_second_execute = fs::read_to_string(&log).unwrap();
    for effect in ["cargo publish", "git push", "gh release create"] {
        assert_eq!(
            after_second_execute.matches(effect).count(),
            effects.matches(effect).count(),
            "a second execute must not repeat `{effect}`"
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
    let receipt = external.path().join("release-receipt.json");
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, false);
    let publishers = FakePublishers {
        bin: &bin,
        log: &log,
        forge_marker: &forge_marker,
        git_trace: &git_trace,
    };

    let first = execute_product(root, &intent, &manifest, &artifacts, &receipt, publishers);
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

    fs::remove_file(&receipt).unwrap();
    let recovered = execute_product(root, &intent, &manifest, &artifacts, &receipt, publishers);
    assert!(
        recovered.status.success(),
        "product rerun failed: {}\n{}",
        String::from_utf8_lossy(&recovered.stderr),
        fs::read_to_string(&log).unwrap_or_else(|_| "<no effect trace>".to_owned()),
    );
    let after_rerun = fs::read_to_string(&log).unwrap();
    assert_eq!(
        after_rerun.matches("gh release upload").count(),
        PRODUCT_ASSETS.len(),
        "a rerun must never upload an existing product artifact again"
    );
    assert!(receipt.exists());
    assert_argv_within_allowlists(&log, &git_trace);
}

#[test]
fn failed_publish_fails_the_run_and_never_tags() {
    let (dir, release_commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let root = dir.path();
    let intent = plan_intent(root, external.path(), &release_commit);
    let receipt = external.path().join("release-receipt.json");
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, true);

    let publishers = FakePublishers {
        bin: &bin,
        log: &log,
        forge_marker: &forge_marker,
        git_trace: &git_trace,
    };

    let output = execute(root, &intent, &receipt, publishers);
    assert!(
        !output.status.success(),
        "a failed registry publish must fail release execution"
    );
    assert!(fs::read_to_string(&log).unwrap().contains("cargo publish"));
    assert!(git(root, &["tag", "--list", "core-crate@0.2.0"]).is_empty());
    assert!(!receipt.exists());
}

#[test]
fn newer_coordinator_executes_and_reruns_an_older_release_source() {
    let (source_dir, release_commit) = release_commit_fixture();
    let source = source_dir.path();
    let (coordinator_dir, coordinator_revision) = coordinator_checkout(source, &release_commit);
    let coordinator = coordinator_dir.path();
    let external = tempfile::tempdir().unwrap();
    let intent = plan_intent_from_source(coordinator, source, external.path(), &release_commit);
    let receipt = external.path().join("release-receipt.json");
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, false);
    let publishers = FakePublishers {
        bin: &bin,
        log: &log,
        forge_marker: &forge_marker,
        git_trace: &git_trace,
    };

    let first = execute_from_coordinator(coordinator, source, &intent, &receipt, publishers);
    assert!(
        first.status.success(),
        "new coordinator execution failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let issued: serde_json::Value =
        serde_json::from_slice(&fs::read(&receipt).expect("initial receipt must exist")).unwrap();
    assert_eq!(issued["envelope"]["orchestrationRevision"], coordinator_revision);
    assert_eq!(issued["envelope"]["releaseSourceRevision"], release_commit);

    fs::remove_file(&receipt).unwrap();
    let effects = fs::read_to_string(&log).unwrap();
    let recovered = execute_from_coordinator(coordinator, source, &intent, &receipt, publishers);
    assert!(
        recovered.status.success(),
        "new coordinator rerun failed: {}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    let after_rerun = fs::read_to_string(&log).unwrap();
    for effect in ["cargo publish", "git push", "gh release create"] {
        assert_eq!(
            after_rerun.matches(effect).count(),
            effects.matches(effect).count(),
            "a rerun must observe the old source's remote effects instead of repeating `{effect}`"
        );
    }
}

#[test]
fn changed_checkout_after_planning_never_reaches_a_publish_boundary() {
    let (dir, release_commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let root = dir.path();
    let intent = plan_intent(root, external.path(), &release_commit);
    let receipt = external.path().join("release-receipt.json");
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, false);

    // This simulates a checkout which changed after plan approval but before
    // the gated execution job.  It must fail before any publisher or forge
    // process is invoked; a stale intent is never a partial authorization.
    fs::write(
        root.join("crates/core/Cargo.toml"),
        "[package]\nname = \"core-crate\"\nversion = \"0.2.1\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let publishers = FakePublishers {
        bin: &bin,
        log: &log,
        forge_marker: &forge_marker,
        git_trace: &git_trace,
    };
    let output = execute(root, &intent, &receipt, publishers);
    assert!(
        !output.status.success(),
        "a changed checkout must invalidate the approved release intent"
    );
    assert!(
        !log.exists(),
        "intent validation must fail before any external release side effect"
    );
    assert!(git(root, &["tag", "--list", "core-crate@0.2.0"]).is_empty());
    assert!(!receipt.exists());
}

/// Committing (unlike the dirty-worktree case above) reaches the fresh-derivation check, so this expects E124.
#[test]
fn committed_change_after_planning_is_rejected_as_a_stale_intent() {
    let (dir, release_commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let root = dir.path();
    let intent = plan_intent(root, external.path(), &release_commit);
    let receipt = external.path().join("release-receipt.json");
    let (bin, log, forge_marker, git_trace) = fake_publishers(external.path(), &release_commit, false);

    // The tree is clean here, so this must fail via the fresh-derivation mismatch, not the dirty-tree check.
    fs::write(
        root.join("crates/core/Cargo.toml"),
        "[package]\nname = \"core-crate\"\nversion = \"0.2.1\"\nedition = \"2021\"\n",
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "further change after approval"]);
    assert!(
        git(root, &["status", "--porcelain"]).is_empty(),
        "the checkout must be clean, not merely re-committed"
    );

    let publishers = FakePublishers {
        bin: &bin,
        log: &log,
        forge_marker: &forge_marker,
        git_trace: &git_trace,
    };
    let output = execute(root, &intent, &receipt, publishers);
    assert!(
        !output.status.success(),
        "a committed change after planning must invalidate the approved release intent"
    );
    assert!(
        diagnostic_codes(&output).iter().any(|code| code == "E124"),
        "a fresh-derivation mismatch must surface E124: {}",
        stderr_of(&output)
    );
    assert!(
        !log.exists(),
        "intent validation must fail before any external release side effect"
    );
    assert!(!receipt.exists());
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

/// No read-only mode exists: planning already writes only `--out`, so `--dry-run` is rejected outright.
#[test]
fn release_plan_dry_run_is_rejected_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let out = root.join("must-not-exist.json");
    let result = callisto(
        root,
        &[
            "--dry-run",
            "release",
            "plan",
            "--package",
            "cargo/example",
            "--out",
            out.to_str().unwrap(),
        ],
    );
    assert!(!result.status.success(), "release plan --dry-run must be rejected");
    assert!(
        diagnostic_codes(&result).iter().any(|code| code == "E217"),
        "release plan --dry-run must surface release_plan_dry_run: {}",
        stderr_of(&result)
    );
    assert!(!out.exists());
}

/// No read-only mode exists: execution either performs remote effects under authorization or does not run.
#[test]
fn release_execute_dry_run_is_rejected_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let receipt = root.join("must-not-exist-receipt.json");
    let result = callisto(
        root,
        &[
            "--dry-run",
            "release",
            "execute",
            "--intent",
            "does-not-exist-intent.json",
            "--receipt",
            receipt.to_str().unwrap(),
            "--orchestration-revision",
            "0000000000000000000000000000000000000000",
        ],
    );
    assert!(!result.status.success(), "release execute --dry-run must be rejected");
    assert!(
        diagnostic_codes(&result).iter().any(|code| code == "E218"),
        "release execute --dry-run must surface release_execute_dry_run: {}",
        stderr_of(&result)
    );
    assert!(!receipt.exists());
}

/// No read-only mode exists: the manifest records what was actually built, never a hypothetical preview.
#[test]
fn release_artifact_manifest_dry_run_is_rejected_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let out = root.join("must-not-exist-manifest.json");
    let result = callisto(
        root,
        &[
            "--dry-run",
            "release",
            "artifact-manifest",
            "--intent",
            "does-not-exist-intent.json",
            "--artifact-dir",
            "does-not-exist-dir",
            "--out",
            out.to_str().unwrap(),
        ],
    );
    assert!(
        !result.status.success(),
        "release artifact-manifest --dry-run must be rejected"
    );
    assert!(
        diagnostic_codes(&result).iter().any(|code| code == "E216"),
        "release artifact-manifest --dry-run must surface release_artifact_manifest_dry_run: {}",
        stderr_of(&result)
    );
    assert!(!out.exists());
}
