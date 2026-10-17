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

mod historical_version_authority {
    use super::*;
    use std::path::Path;

    use callisto_model::{atomic::atomic_write, ApplyPermit, ReleaseDecisionV1};

    struct ManifestRoster(Vec<callisto_model::Package>);

    impl callisto_graph::DependencyResolver for ManifestRoster {
        fn packages(&self) -> impl Iterator<Item = &callisto_model::Package> {
            self.0.iter()
        }

        fn dependencies_of(&self, _: &callisto_model::PackageId) -> impl Iterator<Item = &callisto_model::DepEdge> {
            std::iter::empty()
        }

        fn dependents_of(&self, _: &callisto_model::PackageId) -> impl Iterator<Item = &callisto_model::DepEdge> {
            std::iter::empty()
        }
    }

    fn write(root: &Path, path: &str, content: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        atomic_write(&path, content, &ApplyPermit::force_for_tests()).unwrap();
    }

    fn write_workspace_version(root: &Path, version: callisto_model::Version) {
        let mut manifest = callisto_manifests::WorkspaceCargoResolver::load(&root.join("Cargo.toml")).unwrap();
        let permit = ApplyPermit::force_for_tests();
        manifest.write_version(&version, &permit).unwrap();
        manifest.persist(&permit).unwrap();
    }

    fn inherited_release_fixture(root_authority: bool) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        callisto_fixtures::git::init_repo(root);
        git(
            root,
            &["remote", "add", "origin", "https://github.com/example/inherited.git"],
        );
        write(
            root,
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\nresolver = \"2\"\n\n\
             [workspace.package]\nversion = \"0.1.0\"\n",
        );
        for name in ["a", "b"] {
            write(
                root,
                &format!("crates/{name}/Cargo.toml"),
                &format!("[package]\nname = \"{name}\"\nversion.workspace = true\nedition = \"2021\"\n"),
            );
            write(root, &format!("crates/{name}/src/lib.rs"), "\n");
        }
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "initial workspace"]);
        let init = callisto(root, &["init", "--yes", "--versioning", "independent"]);
        assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));
        let config = fs::read_to_string(root.join("callisto.toml")).unwrap();
        write(
            root,
            "callisto.toml",
            &format!(
                "{config}\n[[package]]\nmatch = \"cargo/a\"\npublish-to = [\"crates-io\"]\n\n\
                 [[package]]\nmatch = \"cargo/b\"\npublish-to = [\"crates-io\"]\n\n\
                 [[fixed-group]]\nname = \"shared\"\nmembers = [\"a\", \"b\"]\n",
            ),
        );
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "configure callisto"]);
        let add = callisto(
            root,
            &["add", "--package", "a:patch", "--summary", "Fix shared version"],
        );
        assert!(add.status.success(), "{}", String::from_utf8_lossy(&add.stderr));
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "add pending changeset"]);
        let version = callisto(
            root,
            &["version", "--no-refresh-lockfiles", "--emit-decision", DECISION_PATH],
        );
        assert!(version.status.success(), "{}", String::from_utf8_lossy(&version.stderr));
        if root_authority {
            write_workspace_version(root, callisto_model::Version::semver(0, 1, 1));
            for name in ["a", "b"] {
                write(
                    root,
                    &format!("crates/{name}/Cargo.toml"),
                    &format!("[package]\nname = \"{name}\"\nversion.workspace = true\nedition = \"2021\"\n"),
                );
            }
        }
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "release shared version"]);
        let commit = git(root, &["rev-parse", "HEAD"]);
        git(root, &["checkout", "--detach", &commit]);
        (dir, commit)
    }

    fn plan(root: &Path, commit: &str, decision_path: &str, out: &Path) -> std::process::Output {
        callisto(
            root,
            &[
                "release",
                "plan",
                "--from-release-commit",
                commit,
                "--decision",
                decision_path,
                "--out",
                out.to_str().unwrap(),
            ],
        )
    }

    #[test]
    fn inherited_version_emit_commit_plan_preserves_the_claimed_roster() {
        let (dir, commit) = inherited_release_fixture(false);
        let root = dir.path();
        let delta = git(
            root,
            &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD^", "HEAD"],
        );
        assert!(!delta.lines().any(|path| path == "Cargo.toml"));
        for name in ["a", "b"] {
            assert!(delta.lines().any(|path| path == format!("crates/{name}/Cargo.toml")));
            assert!(delta.lines().any(|path| path == format!("crates/{name}/CHANGELOG.md")));
        }
        let decision_bytes = fs::read(root.join(DECISION_PATH)).unwrap();
        let decision: ReleaseDecisionV1 = serde_json::from_slice(&decision_bytes).unwrap();
        assert_eq!(decision.entries.len(), 2);
        assert!(decision
            .entries
            .iter()
            .all(|entry| entry.target_version.render() == "0.1.1"));
        let external = tempfile::tempdir().unwrap();
        let intent_path = external.path().join("intent.json");
        let out = plan(root, &commit, "./.callisto/release-decision.json", &intent_path);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let intent: callisto_model::ReleaseIntentV1 = serde_json::from_slice(&fs::read(intent_path).unwrap()).unwrap();
        assert_eq!(intent.decision, decision);
        assert_eq!(fs::read(root.join(DECISION_PATH)).unwrap(), decision_bytes);
    }

    #[test]
    fn inherited_root_authority_change_is_accepted_with_unchanged_member_manifests() {
        let (dir, commit) = inherited_release_fixture(true);
        let root = dir.path();
        let delta = git(
            root,
            &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD^", "HEAD"],
        );
        assert!(delta.lines().any(|path| path == "Cargo.toml"));
        for name in ["a", "b"] {
            assert!(!delta.lines().any(|path| path == format!("crates/{name}/Cargo.toml")));
        }
        let external = tempfile::tempdir().unwrap();
        let intent_path = external.path().join("intent.json");
        let out = plan(root, &commit, DECISION_PATH, &intent_path);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let decision: ReleaseDecisionV1 = serde_json::from_slice(&fs::read(root.join(DECISION_PATH)).unwrap()).unwrap();
        let intent: callisto_model::ReleaseIntentV1 = serde_json::from_slice(&fs::read(intent_path).unwrap()).unwrap();
        assert_eq!(intent.decision, decision);
    }

    #[test]
    fn inherited_release_plan_rejects_an_unclaimed_shared_version_owner() {
        let (dir, _) = inherited_release_fixture(true);
        let root = dir.path();
        let decision: ReleaseDecisionV1 = serde_json::from_slice(&fs::read(root.join(DECISION_PATH)).unwrap()).unwrap();
        let reduced = ReleaseDecisionV1::new(
            decision
                .entries
                .into_iter()
                .filter(|entry| entry.package.name() == "a")
                .collect(),
        )
        .unwrap();
        write(root, DECISION_PATH, &serde_json::to_string_pretty(&reduced).unwrap());
        git(root, &["add", DECISION_PATH]);
        git(root, &["commit", "--amend", "--no-edit"]);
        let commit = git(root, &["rev-parse", "HEAD"]);
        let external = tempfile::tempdir().unwrap();
        let intent = external.path().join("intent.json");
        let out = plan(root, &commit, DECISION_PATH, &intent);
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success());
        assert!(error.contains("E165") && error.contains("unclaimed"), "{error}");
        assert!(!intent.exists());
    }

    #[test]
    fn inherited_release_plan_rejects_a_root_version_that_disagrees_with_the_decision() {
        let (dir, _) = inherited_release_fixture(true);
        let root = dir.path();
        write_workspace_version(root, callisto_model::Version::semver(0, 9, 0));
        git(root, &["add", "Cargo.toml"]);
        git(root, &["commit", "--amend", "--no-edit"]);
        let commit = git(root, &["rev-parse", "HEAD"]);
        let external = tempfile::tempdir().unwrap();
        let intent = external.path().join("intent.json");
        let out = plan(root, &commit, DECISION_PATH, &intent);
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success());
        assert!(error.contains("E165") && error.contains("manifest version"), "{error}");
        assert!(!intent.exists());
    }

    #[test]
    fn release_commit_plan_rejects_escaping_and_control_character_paths() {
        let (dir, commit) = inherited_release_fixture(false);
        let external = tempfile::tempdir().unwrap();
        for path in [
            "../release-decision.json",
            ".callisto/../../release-decision.json",
            "bad\npath",
            "bad\rpath",
        ] {
            let intent = external.path().join("intent.json");
            let out = plan(dir.path(), &commit, path, &intent);
            let error = String::from_utf8_lossy(&out.stderr);
            assert!(!out.status.success(), "accepted malicious decision path {path:?}");
            assert!(
                error.contains("E164") || error.contains("E226"),
                "unexpected path diagnostic: {error}",
            );
            assert!(!intent.exists());
        }
    }

    #[test]
    fn release_commit_verification_normalizes_member_paths_and_rejects_unsafe_roster_paths() {
        use callisto_graph::DependencyResolver;

        let (dir, raw_commit) = inherited_release_fixture(false);
        let root = dir.path();
        let commit = callisto_model::CommitSha::parse(&raw_commit).unwrap();
        for unsafe_path in [
            None,
            Some("../Cargo.toml"),
            Some("/Cargo.toml"),
            Some("bad\nCargo.toml"),
        ] {
            let runner = callisto_fixtures::git::GitRunner;
            let locator = callisto_graph::locate::IgnoreWalkLocator::new(root);
            let loaded = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();
            let mut packages: Vec<_> = loaded.graph.packages().cloned().collect();
            for package in &mut packages {
                for manifest in &mut package.manifests {
                    manifest.path = format!("./{}", manifest.path.display()).into();
                }
                if let Some(changelog) = &mut package.changelog {
                    *changelog = format!("./{}", changelog.display()).into();
                }
            }
            if let Some(path) = unsafe_path {
                packages[0].manifests[0].path = path.into();
            }
            let workspace = callisto_graph::Workspace {
                root: loaded.root,
                config: loaded.config,
                graph: ManifestRoster(packages),
                tags: loaded.tags,
                git: loaded.git,
                runner: loaded.runner,
                manifest_cache: loaded.manifest_cache,
                identity: loaded.identity,
            };
            let result = callisto_graph::commands::derive_release_commit_decision(
                &workspace,
                &commit,
                Path::new("./.callisto/release-decision.json"),
            );
            match unsafe_path {
                None => {
                    let expected: ReleaseDecisionV1 =
                        serde_json::from_slice(&fs::read(root.join(DECISION_PATH)).unwrap()).unwrap();
                    assert_eq!(result.unwrap(), expected);
                }
                Some("../Cargo.toml") => assert!(matches!(
                    result,
                    Err(callisto_graph::GraphError::Model(
                        callisto_model::ModelError::PathTraversal { .. }
                    )),
                )),
                Some("/Cargo.toml") => assert!(matches!(
                    result,
                    Err(callisto_graph::GraphError::Model(
                        callisto_model::ModelError::AbsolutePath { .. }
                    )),
                )),
                Some(_) => assert!(matches!(result, Err(callisto_graph::GraphError::ReleaseCommand { .. }))),
            }
        }
    }
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

mod static_python_release {
    use super::*;
    use std::{os::unix::fs::PermissionsExt, path::Path};

    use callisto_model::{
        atomic::atomic_write, ApplyPermit, ProviderEvidenceV1, ProviderObservationV1, ReleaseDecisionV1,
        ReleaseIntentV1, ReleaseReceiptV1, VersionGrammar,
    };

    const PACKAGE: &str = "pypi/stable-python";
    const REMOTE: &str = "https://github.com/example/stable-python.git";
    const TAG: &str = "stable-python@0.2.0";

    fn write_fixture(path: &Path, content: &str) {
        atomic_write(path, content, &ApplyPermit::force_for_tests()).unwrap();
    }

    fn release_fixture() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        callisto_fixtures::git::init_repo(root);
        git(root, &["remote", "add", "origin", REMOTE]);
        write_fixture(
            &root.join("Cargo.toml"),
            "[workspace]\nmembers = []\nresolver = \"2\"\n",
        );
        write_fixture(
            &root.join("pkg/pyproject.toml"),
            "[project]\nname = \"stable-python\"\nversion = \"0.1.0\"\n",
        );
        write_fixture(&root.join(".gitignore"), "/pkg/dist/\n");
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "initial Python workspace"]);

        let init = callisto(root, &["init", "--yes", "--versioning", "independent"]);
        assert!(init.status.success(), "Python init failed: {}", stderr_of(&init));
        let config_path = root.join("callisto.toml");
        let config = fs::read_to_string(&config_path).unwrap();
        write_fixture(
            &config_path,
            &format!("{config}\n[[package]]\nmatch = \"{PACKAGE}\"\npublish-to = [\"pypi\", \"github-release\"]\n"),
        );
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "configure Python release"]);

        let add = callisto(
            root,
            &[
                "add",
                "--package",
                "stable-python:minor",
                "--summary",
                "Ship stable Python release",
            ],
        );
        assert!(add.status.success(), "Python changeset failed: {}", stderr_of(&add));
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "add Python changeset"]);
        let pending = fs::read_dir(root.join(".changeset"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension().is_some_and(|extension| extension == "md")
                    && path.file_name().is_some_and(|name| name != "README.md")
            })
            .collect::<Vec<_>>();
        assert_eq!(pending.len(), 1, "the parent must contain the actual release input");

        let applied = callisto(
            root,
            &["version", "--no-refresh-lockfiles", "--emit-decision", DECISION_PATH],
        );
        assert!(
            applied.status.success(),
            "Python version apply failed: {}",
            stderr_of(&applied)
        );
        assert_eq!(
            fs::read_to_string(root.join("pkg/pyproject.toml")).unwrap(),
            "[project]\nname = \"stable-python\"\nversion = \"0.2.0\"\n"
        );
        assert!(fs::read_to_string(root.join("pkg/CHANGELOG.md"))
            .unwrap()
            .contains("Ship stable Python release"));
        for changeset in pending {
            assert!(!changeset.exists(), "version must consume its pending changeset");
        }
        git(root, &["add", "-A"]);
        git(root, &["commit", "-m", "release stable-python 0.2.0"]);
        let revision = git(root, &["rev-parse", "HEAD"]);
        git(root, &["checkout", "--detach", &revision]);
        assert!(git(root, &["status", "--porcelain"]).is_empty());
        (dir, revision)
    }

    fn plan(root: &Path, revision: &str, out: &Path) -> std::process::Output {
        callisto(
            root,
            &[
                "release",
                "plan",
                "--from-release-commit",
                revision,
                "--decision",
                DECISION_PATH,
                "--out",
                out.to_str().unwrap(),
            ],
        )
    }

    fn assert_python_intent(path: &Path) -> ReleaseIntentV1 {
        let bytes = fs::read(path).unwrap();
        let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(wire["decision"]["entries"][0]["package"], PACKAGE);
        assert_eq!(wire["decision"]["entries"][0]["targetVersion"], "0.2.0");
        let intent: ReleaseIntentV1 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(intent.decision.entries.len(), 1);
        assert_eq!(
            intent.decision.entries[0].target_version.grammar(),
            VersionGrammar::Pep440
        );
        assert_eq!(intent.operations.len(), 4);
        for operation in &intent.operations {
            let id = operation.id();
            assert_eq!(id.package.to_string(), PACKAGE);
            assert_eq!(id.version.grammar(), VersionGrammar::Pep440);
            assert_eq!(id.version.render(), "0.2.0");
            for prerequisite in operation.prerequisites() {
                assert_eq!(prerequisite.version.grammar(), VersionGrammar::Pep440);
            }
        }
        assert_eq!(serde_json::to_value(&intent).unwrap(), wire);
        intent
    }

    fn replace_committed_decision(root: &Path, decision: &serde_json::Value) -> String {
        write_fixture(
            &root.join(DECISION_PATH),
            &serde_json::to_string_pretty(decision).unwrap(),
        );
        git(root, &["add", DECISION_PATH]);
        git(root, &["commit", "--amend", "--no-edit"]);
        git(root, &["rev-parse", "HEAD"])
    }

    #[test]
    fn python_stable_emit_decision_commit_plan_preserves_native_versions() {
        let (dir, revision) = release_fixture();
        let external = tempfile::tempdir().unwrap();
        let decision_bytes = fs::read(dir.path().join(DECISION_PATH)).unwrap();
        let wire: serde_json::Value = serde_json::from_slice(&decision_bytes).unwrap();
        assert_eq!(wire["schemaVersion"], 2);
        assert_eq!(wire["entries"][0]["targetVersion"], "0.2.0");
        let out = external.path().join("intent.json");
        let result = plan(dir.path(), &revision, &out);
        assert!(
            result.status.success(),
            "Python committed release plan failed: {}",
            stderr_of(&result)
        );
        let intent = assert_python_intent(&out);
        let decision: ReleaseDecisionV1 = serde_json::from_slice(&decision_bytes).unwrap();
        assert_eq!(decision.entries[0].target_version.grammar(), VersionGrammar::Pep440);
        assert_eq!(intent.decision, decision);
        assert_eq!(serde_json::to_value(decision).unwrap(), wire);
        assert_eq!(fs::read(dir.path().join(DECISION_PATH)).unwrap(), decision_bytes);
    }

    #[test]
    fn python_stable_committed_decision_accepts_schema_one_and_two() {
        let (dir, _) = release_fixture();
        let external = tempfile::tempdir().unwrap();
        let mut wire: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join(DECISION_PATH)).unwrap()).unwrap();
        let digest = wire["digest"].clone();
        assert_eq!(
            wire["entries"][0]["reasons"],
            serde_json::json!([{ "kind": "changeset" }])
        );
        for schema in [1, 2] {
            wire["schemaVersion"] = schema.into();
            let revision = replace_committed_decision(dir.path(), &wire);
            let out = external.path().join(format!("schema-{schema}-intent.json"));
            let result = plan(dir.path(), &revision, &out);
            assert!(
                result.status.success(),
                "Python decision schema {schema} rejected: {}",
                stderr_of(&result)
            );
            let intent = assert_python_intent(&out);
            assert_eq!(serde_json::to_value(intent.decision.digest).unwrap(), digest);
        }
    }

    #[test]
    fn python_stable_release_plan_rejects_unsupported_schema_and_tampered_digest() {
        let (dir, _) = release_fixture();
        let external = tempfile::tempdir().unwrap();
        let original: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join(DECISION_PATH)).unwrap()).unwrap();
        for (case, field, value, expected) in [
            (
                "schema",
                "schemaVersion",
                serde_json::json!(255),
                "unsupported release decision schema version",
            ),
            (
                "digest",
                "digest",
                serde_json::json!("0".repeat(64)),
                "release decision digest does not match",
            ),
        ] {
            let mut wire = original.clone();
            wire[field] = value;
            let revision = replace_committed_decision(dir.path(), &wire);
            let out = external.path().join(format!("rejected-{case}-intent.json"));
            let result = plan(dir.path(), &revision, &out);
            assert!(!result.status.success(), "invalid Python {case} must fail");
            assert!(
                stderr_of(&result).contains(expected),
                "unexpected {case} diagnostic: {}",
                stderr_of(&result)
            );
            assert!(diagnostic_codes(&result).iter().any(|code| code == "E166"));
            assert!(!out.exists(), "invalid Python {case} must not authorize execution");
        }
    }

    #[test]
    fn python_stable_release_plan_rejects_digest_valid_foreign_ecosystem_roster() {
        let (dir, _) = release_fixture();
        let external = tempfile::tempdir().unwrap();
        let mut wire: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join(DECISION_PATH)).unwrap()).unwrap();
        wire["entries"][0]["package"] = "npm/stable-python".into();
        let entry: callisto_model::ReleaseDecisionEntry = serde_json::from_value(wire["entries"][0].clone()).unwrap();
        let forged = ReleaseDecisionV1::new(vec![entry]).unwrap();
        let wire = serde_json::to_value(forged).unwrap();
        let decoded: ReleaseDecisionV1 = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(decoded.entries[0].target_version.grammar(), VersionGrammar::SemVer);
        let revision = replace_committed_decision(dir.path(), &wire);
        let out = external.path().join("foreign-ecosystem-intent.json");
        let result = plan(dir.path(), &revision, &out);
        assert!(
            !result.status.success(),
            "an npm roster cannot authorize the applied Python manifest"
        );
        assert!(
            diagnostic_codes(&result).iter().any(|code| code == "E165"),
            "this negative must reach the commit/domain cross-check: {}",
            stderr_of(&result)
        );
        assert!(!out.exists());
    }

    fn python_publishers(external: &Path, revision: &str) -> Rig {
        let mut rig = Rig::new(external, revision);
        let bare = bare_remote(external);
        rig.real_git_push(&bare, REMOTE);
        rig.set(
            "CALLISTO_TEST_PYPI_MARKER",
            external.join("pypi-published").to_str().unwrap(),
        );
        rig.set(
            "CALLISTO_TEST_PYPI_CALLS",
            external.join("pypi-calls.log").to_str().unwrap(),
        );
        for (program, body) in [
            ("curl", PYPI_CURL),
            ("python3", PYTHON_BUILD),
            ("python", PYTHON_BUILD),
            ("twine", TWINE_UPLOAD),
        ] {
            let path = rig.bin.join(program);
            write_fixture(&path, body);
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        rig
    }

    fn assert_python_receipt(path: &Path, intent: &ReleaseIntentV1, outcome: &str, revision: &str) {
        let bytes = fs::read(path).unwrap();
        let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(wire["envelope"]["releaseSourceRevision"], revision);
        assert_eq!(wire["envelope"]["orchestrationRevision"], revision);
        assert_eq!(wire["intentDigest"], serde_json::to_value(intent.digest()).unwrap());
        let outcomes = wire["outcomes"].as_array().unwrap();
        assert_eq!(outcomes.len(), intent.operations.len());
        for entry in outcomes {
            assert_eq!(entry["operation"]["package"], PACKAGE);
            assert_eq!(entry["operation"]["version"], "0.2.0");
            assert_eq!(entry["outcome"]["kind"], outcome);
        }
        let receipt: ReleaseReceiptV1 = serde_json::from_slice(&bytes).unwrap();
        let roundtrip = serde_json::to_value(&receipt).unwrap();
        assert_eq!(roundtrip, wire);
        let mut registry_observations = 0;
        for observation in wire["observations"].as_array().unwrap() {
            let native: callisto_model::ReleaseOperationObservationV1 =
                serde_json::from_value(observation.clone()).unwrap();
            assert_eq!(native.operation().version.grammar(), VersionGrammar::Pep440);
            if let ProviderObservationV1::Exact {
                evidence: ProviderEvidenceV1::RegistryVersion { version, .. },
            } = native.observation()
            {
                registry_observations += 1;
                assert_eq!(version.grammar(), VersionGrammar::Pep440);
                assert_eq!(version.render(), "0.2.0");
            }
        }
        assert_eq!(registry_observations, 1);
    }

    #[test]
    fn python_stable_release_executes_and_reruns_from_provider_evidence() {
        let (dir, revision) = release_fixture();
        let external = tempfile::tempdir().unwrap();
        let intent_path = external.path().join("intent.json");
        let result = plan(dir.path(), &revision, &intent_path);
        assert!(result.status.success(), "Python plan failed: {}", stderr_of(&result));
        let intent = assert_python_intent(&intent_path);
        let rig = python_publishers(external.path(), &revision);
        let receipt = external.path().join("receipt.json");
        let first = execute_rig(dir.path(), &intent_path, &receipt, &rig, TAG, &[], None);
        assert!(first.status.success(), "Python execute failed: {}", stderr_of(&first));
        assert_python_receipt(&receipt, &intent, "published", &revision);
        assert_eq!(
            git(dir.path(), &["rev-parse", &format!("refs/tags/{TAG}^{{commit}}")]),
            revision
        );
        assert_eq!(git(dir.path(), &["cat-file", "-t", &format!("refs/tags/{TAG}")]), "tag");
        assert_eq!(
            git(
                &external.path().join("remote.git"),
                &["rev-parse", &format!("refs/tags/{TAG}^{{commit}}")]
            ),
            revision
        );
        let calls_path = external.path().join("pypi-calls.log");
        let calls = fs::read_to_string(&calls_path).unwrap();
        assert_eq!(calls.lines().filter(|line| line.starts_with("python ")).count(), 1);
        assert_eq!(calls.lines().filter(|line| line.starts_with("twine ")).count(), 1);
        assert_eq!(rig.log_count("git push"), 1);
        assert_eq!(rig.log_count("gh release create"), 1);
        assert_eq!(rig.log_count("gh release edit"), 1);
        assert_argv_within_allowlists(&rig.log, &rig.git_trace);

        fs::remove_file(&receipt).unwrap();
        let second = execute_rig(dir.path(), &intent_path, &receipt, &rig, TAG, &[], None);
        assert!(second.status.success(), "Python rerun failed: {}", stderr_of(&second));
        assert_python_receipt(&receipt, &intent, "alreadySatisfied", &revision);
        let recovered_calls = fs::read_to_string(&calls_path).unwrap();
        let count = |log: &str, tool: &str| log.lines().filter(|line| line.starts_with(tool)).count();
        assert_eq!(
            count(&recovered_calls, "twine "),
            count(&calls, "twine "),
            "a complete release uploads nothing on a rerun"
        );
        assert_eq!(
            count(&recovered_calls, "python "),
            count(&calls, "python ") + 1,
            "a rerun builds once to know which files the release declares"
        );
        assert!(recovered_calls.matches("curl ").count() > calls.matches("curl ").count());
        for effect in ["git push", "gh release create", "gh release edit"] {
            assert_eq!(rig.log_count(effect), 1, "rerun repeated {effect}");
        }
        assert_argv_within_allowlists(&rig.log, &rig.git_trace);
        assert!(git(dir.path(), &["status", "--porcelain"]).is_empty());
    }

    /// One file per name in `$CALLISTO_TEST_PYPI_STATE`: the index serves exactly what was uploaded.
    const PYPI_STATE_CURL: &str = r#"#!/bin/sh
set -eu
printf 'curl\n' >> "$CALLISTO_TEST_PYPI_CALLS"
set -- "$CALLISTO_TEST_PYPI_STATE"/*
if [ ! -e "$1" ]; then
  printf 'HTTP/2 404 Not Found\r\n\r\n'
  exit 0
fi
printf 'HTTP/2 200 OK\r\ncontent-type: application/vnd.pypi.simple.v1+json\r\n\r\n{"meta":{"api-version":"1.0"},"name":"stable-python","files":['
sep=''
for f in "$@"; do
  printf '%s{"filename":"%s","url":"https://files.example.invalid/x","hashes":{},"yanked":false}' "$sep" "$(basename "$f")"
  sep=','
done
printf ']}'
"#;

    /// Uploads each named file in order, and dies with a 503 before the second one when told to.
    const PYPI_STATE_TWINE: &str = r#"#!/bin/sh
set -eu
[ "$1" = upload ] || exit 64
shift
printf 'twine %s\n' "$#" >> "$CALLISTO_TEST_PYPI_CALLS"
sent=0
for f in "$@"; do
  name=$(basename "$f")
  [ -f "$f" ] || exit 65
  [ ! -e "$CALLISTO_TEST_PYPI_STATE/$name" ] || { echo 'HTTPError: 400 File already exists' >&2; exit 1; }
  if [ "$sent" = 1 ] && [ -e "$CALLISTO_TEST_PYPI_FAIL" ]; then
    echo 'HTTPError: 503 Service Unavailable' >&2
    exit 1
  fi
  : > "$CALLISTO_TEST_PYPI_STATE/$name"
  printf 'Uploading %s\n' "$name"
  sent=$((sent + 1))
done
"#;

    #[test]
    fn python_release_resumes_after_a_partial_upload_and_uploads_only_the_missing_file() {
        let (dir, revision) = release_fixture();
        let external = tempfile::tempdir().unwrap();
        let intent_path = external.path().join("intent.json");
        let result = plan(dir.path(), &revision, &intent_path);
        assert!(result.status.success(), "Python plan failed: {}", stderr_of(&result));
        let intent = assert_python_intent(&intent_path);
        let state = external.path().join("pypi-state");
        fs::create_dir_all(&state).unwrap();
        let fail = external.path().join("pypi-fail");
        fs::write(&fail, "").unwrap();

        let mut rig = python_publishers(external.path(), &revision);
        rig.set("CALLISTO_TEST_PYPI_STATE", state.to_str().unwrap());
        rig.set("CALLISTO_TEST_PYPI_FAIL", fail.to_str().unwrap());
        for (program, body) in [("curl", PYPI_STATE_CURL), ("twine", PYPI_STATE_TWINE)] {
            let path = rig.bin.join(program);
            write_fixture(&path, body);
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let receipt = external.path().join("receipt.json");
        let first = execute_rig(dir.path(), &intent_path, &receipt, &rig, TAG, &[], None);
        assert!(!first.status.success(), "the dying upload must fail the run");
        assert!(!receipt.exists(), "a failed run issues no receipt");
        let uploaded: Vec<_> = fs::read_dir(&state).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(uploaded.len(), 1, "exactly one file reached the index: {uploaded:?}");
        assert_eq!(
            rig.log_count("git push"),
            0,
            "no tag is pushed before the release is complete"
        );

        fs::remove_file(&fail).unwrap();
        let second = execute_rig(dir.path(), &intent_path, &receipt, &rig, TAG, &[], None);
        assert!(second.status.success(), "the rerun failed: {}", stderr_of(&second));
        assert_python_receipt(&receipt, &intent, "published", &revision);
        let calls = fs::read_to_string(external.path().join("pypi-calls.log")).unwrap();
        let twine: Vec<_> = calls.lines().filter(|line| line.starts_with("twine ")).collect();
        assert_eq!(
            twine,
            ["twine 2", "twine 1"],
            "the first run offered both files, the rerun only the missing one: {calls}"
        );
        assert_eq!(fs::read_dir(&state).unwrap().count(), 2);
        assert_eq!(rig.log_count("git push"), 1);
    }

    const PYPI_CURL: &str = r#"#!/bin/sh
set -eu
[ "$#" = 5 ] && [ "$1" = '-sS' ] && [ "$2" = '-i' ] && [ "$3" = '-H' ] \
  && [ "$4" = 'Accept: application/vnd.pypi.simple.v1+json' ] \
  && [ "$5" = 'https://pypi.org/simple/stable-python/' ] || exit 64
printf 'curl %s\n' "$*" >> "$CALLISTO_TEST_PYPI_CALLS"
if [ -f "$CALLISTO_TEST_PYPI_MARKER" ]; then
  printf 'HTTP/2 200 OK\r\ncontent-type: application/vnd.pypi.simple.v1+json\r\n\r\n'
  cat "$CALLISTO_TEST_PYPI_MARKER"
else
  printf 'HTTP/2 404 Not Found\r\n\r\n'
fi
"#;

    const PYTHON_BUILD: &str = r#"#!/bin/sh
set -eu
[ "$#" = 6 ] && [ "$1" = '-m' ] && [ "$2" = build ] && [ "$3" = '--sdist' ] \
  && [ "$4" = '--wheel' ] && [ "$5" = '--outdir' ] && [ -d "$6" ] || exit 64
grep -Fx 'name = "stable-python"' pyproject.toml >/dev/null
grep -Fx 'version = "0.2.0"' pyproject.toml >/dev/null
printf 'python build\n' >> "$CALLISTO_TEST_PYPI_CALLS"
printf 'fixture wheel\n' > "$6/stable_python-0.2.0-py3-none-any.whl"
printf 'fixture sdist\n' > "$6/stable_python-0.2.0.tar.gz"
"#;

    const TWINE_UPLOAD: &str = r#"#!/bin/sh
set -eu
[ "$#" = 3 ] && [ "$1" = upload ] || exit 64
case "$2" in */stable_python-0.2.0-py3-none-any.whl) ;; *) exit 64 ;; esac
case "$3" in */stable_python-0.2.0.tar.gz) ;; *) exit 64 ;; esac
[ -f "$2" ] && [ -f "$3" ] || exit 65
[ ! -e "$CALLISTO_TEST_PYPI_MARKER" ] || exit 66
printf 'twine upload\n' >> "$CALLISTO_TEST_PYPI_CALLS"
printf '%s\n' '{"meta":{"api-version":"1.0"},"name":"stable-python","files":[{"filename":"stable_python-0.2.0-py3-none-any.whl","url":"https://files.example.invalid/stable_python-0.2.0-py3-none-any.whl","hashes":{},"yanked":false},{"filename":"stable_python-0.2.0.tar.gz","url":"https://files.example.invalid/stable_python-0.2.0.tar.gz","hashes":{},"yanked":false}]}' > "$CALLISTO_TEST_PYPI_MARKER"
printf 'Uploading stable_python-0.2.0-py3-none-any.whl\nUploading stable_python-0.2.0.tar.gz\n'
"#;
}
