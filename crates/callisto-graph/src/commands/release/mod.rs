//! Fresh, graph-owned authorization for durable release intents.
//!
//! This module intentionally takes a root, locator, and runner rather than a
//! `Workspace`. A workspace caches parsed manifests and config for a normal
//! command invocation; accepting one here would make a previously observed
//! graph look current after the filesystem changed.
//!
//! The split is by responsibility: `derive` turns a decision into an
//! immutable intent, `binding` canonicalizes every destination, `provider`
//! is the port through which all remote facts and effects flow, and
//! `capability` is the validated authorization the executor holds.

/// Reason carried by [`crate::GraphError::ReleaseIntentStale`] (E124); real reasons are constructible only from this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaleReason {
    kind: StaleKind,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
enum StaleKind {
    #[error("the re-observed Git commit trust evidence no longer matches what the intent was validated against")]
    TrustEvidenceChanged,
    #[error("the re-observed source identity no longer matches what the intent was validated against")]
    SourceIdentityChanged,
    #[error("the re-observed Git push remote no longer matches what the intent was validated against")]
    GitRemoteChanged,
    #[error("a fresh derivation from the current workspace no longer matches the intent that was validated")]
    IntentDiffersFromFreshDerivation,
}

impl std::fmt::Display for StaleReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.kind, formatter)
    }
}

impl StaleReason {
    fn trust_evidence_changed() -> Self {
        StaleReason {
            kind: StaleKind::TrustEvidenceChanged,
        }
    }

    fn source_identity_changed() -> Self {
        StaleReason {
            kind: StaleKind::SourceIdentityChanged,
        }
    }

    fn git_remote_changed() -> Self {
        StaleReason {
            kind: StaleKind::GitRemoteChanged,
        }
    }

    fn intent_differs_from_fresh_derivation() -> Self {
        StaleReason {
            kind: StaleKind::IntentDiffersFromFreshDerivation,
        }
    }
}

mod binding;
mod capability;
mod derive;
mod github;
pub(crate) mod provider;

use super::release_artifacts;

pub use capability::{
    build_release_intent, build_release_intent_with_artifacts, validate_release_intent, ValidatedReleaseIntent,
};
#[cfg(test)]
pub(crate) use derive::canonical_operation_order;
pub use derive::ArtifactBuildPolicy;
pub(crate) use provider::policy::timeouts;
pub use provider::{ReleasePreflight, ReleaseProviderSet};

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use callisto_model::{
        CommandError, CommandOutput, CommandRunner, Ecosystem, ExecutionTrustProfileV1, ReleaseDecisionV1,
        ReleasePackageId, ReleaseProfileId, SourceIdentity, Version, VersionGrammar,
    };

    use super::provider::PreparedOperation;
    use super::*;
    use crate::error::RemoteConflict;
    use crate::{DependencyResolver, GraphError, Workspace};

    pub(crate) struct RealGitRunner;
    impl CommandRunner for RealGitRunner {
        fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
            let output = std::process::Command::new(program)
                .args(args)
                .current_dir(cwd)
                .output()
                .map_err(|error| CommandError::Io {
                    program: program.to_string(),
                    message: error.to_string(),
                })?;
            Ok(CommandOutput {
                exit_code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
    }

    /// Real git except `ls-remote`, whose answer is read from a file. The
    /// fixture's `origin` is a real GitHub URL that must never be contacted.
    struct StubbedRemote {
        ls_remote: std::path::PathBuf,
    }
    impl CommandRunner for StubbedRemote {
        fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
            if args.first() == Some(&"ls-remote") {
                return Ok(CommandOutput {
                    exit_code: Some(0),
                    stdout: std::fs::read_to_string(&self.ls_remote).unwrap_or_default(),
                    stderr: String::new(),
                });
            }
            RealGitRunner.run(program, args, cwd)
        }
    }

    fn git_stdout(root: &Path, args: &[&str]) -> String {
        RealGitRunner.run("git", args, root).unwrap().stdout.trim().to_owned()
    }

    pub(crate) fn fixture() -> (tempfile::TempDir, RealGitRunner) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"release-fixture\"\nversion = \"1.2.3\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("callisto.toml"),
            "[[package]]\nmatch = \"release-fixture\"\npublish-to = [\"crates-io\"]\n",
        )
        .unwrap();
        for args in [
            ["init", "-q"].as_slice(),
            ["config", "user.email", "test@example.com"].as_slice(),
            ["config", "user.name", "Test"].as_slice(),
            ["config", "commit.gpgsign", "false"].as_slice(),
            [
                "remote",
                "add",
                "origin",
                "https://github.com/example/release-fixture.git",
            ]
            .as_slice(),
            ["add", "."].as_slice(),
            ["commit", "-q", "-m", "fixture"].as_slice(),
            ["checkout", "--detach", "-q", "HEAD"].as_slice(),
        ] {
            assert!(std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success());
        }
        (dir, RealGitRunner)
    }

    pub(crate) fn decision() -> ReleaseDecisionV1 {
        ReleaseDecisionV1::new(vec![callisto_model::ReleaseDecisionEntry {
            package: ReleasePackageId::new(Ecosystem::Cargo, "release-fixture").unwrap(),
            target_version: Version::parse("1.2.3", VersionGrammar::SemVer).unwrap(),
            reasons: vec![callisto_model::ReleaseInclusionReason::ExplicitSelection],
        }])
        .unwrap()
    }

    /// Regression coverage for the callisto-graph derivation-drift finding:
    /// `derive_release_decision` and this module's `derive_release_inputs`
    /// each independently mapped a package's canonical manifests to
    /// `ReleasePackageId`s -- one via a plain per-manifest map, the other via
    /// a `BTreeSet<Ecosystem>` dedup step -- before both were migrated onto
    /// `release_decision::release_package_ids`. A dual-identity package (one
    /// directory, a Cargo manifest and an npm manifest, Case D) is exactly
    /// the shape that would have silently diverged had the two derivations
    /// disagreed: this proves the roster `derive_release_decision` computes
    /// and the snapshot `build_release_intent` derives from it agree on the
    /// exact same two `ReleasePackageId`s.
    #[test]
    fn dual_identity_package_release_ids_agree_between_decision_and_intent_derivation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"release-fixture\"\nversion = \"1.2.3\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"name":"release-fixture","version":"1.2.3"}"#,
        )
        .unwrap();
        // publish-to = [] keeps this test scoped to identity derivation --
        // no tag/registry operations, no git remote requirement.
        std::fs::write(
            dir.path().join("callisto.toml"),
            "[[package]]\nmatch = \"release-fixture\"\npublish-to = []\n",
        )
        .unwrap();
        for args in [
            ["init", "-q"].as_slice(),
            ["config", "user.email", "test@example.com"].as_slice(),
            ["config", "user.name", "Test"].as_slice(),
            ["config", "commit.gpgsign", "false"].as_slice(),
            ["add", "."].as_slice(),
            ["commit", "-q", "-m", "fixture"].as_slice(),
            ["checkout", "--detach", "-q", "HEAD"].as_slice(),
        ] {
            assert!(std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success());
        }
        let runner = RealGitRunner;
        let locator = crate::IgnoreWalkLocator::new(dir.path());

        let workspace = Workspace::load(dir.path().to_path_buf(), &locator, &runner).unwrap();
        let package_id = workspace
            .graph
            .packages()
            .next()
            .expect("workspace should discover the dual-identity package")
            .id
            .clone();
        assert_eq!(
            workspace.graph.packages().count(),
            1,
            "the Cargo and npm manifest must merge into one Case D package"
        );

        let plan = crate::VersionPlan {
            bumps: vec![crate::PlannedBump {
                package: package_id,
                from: Version::parse("1.2.3", VersionGrammar::SemVer).unwrap(),
                to: Version::parse("1.3.0", VersionGrammar::SemVer).unwrap(),
                severity: callisto_model::Severity::Minor,
                governed_by: None,
                reason: None,
                writes: vec![],
            }],
            ..Default::default()
        };
        let decision = crate::commands::release_decision::derive_release_decision(&workspace, &plan).unwrap();
        let decided_ids: std::collections::BTreeSet<_> =
            decision.entries.iter().map(|entry| entry.package.clone()).collect();
        assert_eq!(
            decided_ids,
            std::collections::BTreeSet::from([
                ReleasePackageId::new(Ecosystem::Cargo, "release-fixture").unwrap(),
                ReleasePackageId::new(Ecosystem::Npm, "release-fixture").unwrap(),
            ]),
            "derive_release_decision must produce exactly the cargo and npm identities for the dual-identity package"
        );

        let intent = build_release_intent(
            dir.path(),
            &locator,
            &runner,
            &decision,
            ReleaseProfileId::production(),
            ExecutionTrustProfileV1::GitCommit,
        )
        .unwrap();
        let intent_ids: std::collections::BTreeSet<_> = intent
            .snapshot
            .packages
            .iter()
            .map(|input| input.package.clone())
            .collect();
        assert_eq!(
            intent_ids, decided_ids,
            "derive_release_inputs must agree with derive_release_decision on release package identity"
        );
    }

    /// A Case D package's npm identity is its package.json `name`, not the crate name.
    #[test]
    fn case_d_npm_release_identity_uses_the_package_json_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"addon\"\nversion = \"1.2.3\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"name":"@s/addon","version":"1.2.3"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("callisto.toml"),
            "[[package]]\nmatch = \"addon\"\npublish-to = []\n",
        )
        .unwrap();
        let runner = RealGitRunner;
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let workspace = Workspace::load(dir.path().to_path_buf(), &locator, &runner).unwrap();
        let package = workspace.graph.packages().next().unwrap();
        assert_eq!(workspace.graph.packages().count(), 1);

        let ids = crate::commands::release_decision::release_package_ids(&workspace.identity, package).unwrap();
        assert_eq!(
            ids.into_iter().collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from([
                ReleasePackageId::new(Ecosystem::Cargo, "addon").unwrap(),
                ReleasePackageId::new(Ecosystem::Npm, "@s/addon").unwrap(),
            ]),
        );
    }

    /// oxc-react-docgen's shape: an npm owner whose platform packages live in
    /// `npm/<platform>` directories outside the pnpm workspace globs.
    fn platform_owner_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let write = |rel: &str, body: &str| {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        write("pnpm-workspace.yaml", "packages:\n  - \"packages/*\"\n");
        write("pnpm-lock.yaml", "lockfileVersion: '9.0'\n");
        write(
            "packages/cli/package.json",
            r#"{"name":"@s/cli","version":"0.1.0","publishConfig":{"access":"public"},"optionalDependencies":{"@s/cli-darwin-arm64":"0.1.0","@s/cli-linux-x64-gnu":"0.1.0"}}"#,
        );
        for (suffix, os, cpu) in [("darwin-arm64", "darwin", "arm64"), ("linux-x64-gnu", "linux", "x64")] {
            write(
                &format!("packages/cli/npm/{suffix}/package.json"),
                &format!(r#"{{"name":"@s/cli-{suffix}","version":"0.1.0","os":["{os}"],"cpu":["{cpu}"]}}"#),
            );
        }
        write("callisto.toml", "");
        for args in [
            ["init", "-q"].as_slice(),
            ["config", "user.email", "test@example.com"].as_slice(),
            ["config", "user.name", "Test"].as_slice(),
            ["config", "commit.gpgsign", "false"].as_slice(),
            ["remote", "add", "origin", "https://github.com/example/platforms.git"].as_slice(),
            ["add", "."].as_slice(),
            ["commit", "-q", "-m", "fixture"].as_slice(),
            ["checkout", "--detach", "-q", "HEAD"].as_slice(),
        ] {
            assert!(std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success());
        }
        dir
    }

    /// §M.6.1 Case E: each attached platform package is one `PlatformPublish`
    /// under its owner, published by directory before the owner, with no tag,
    /// forge, or artifact operation of its own.
    #[test]
    fn platform_packages_publish_by_directory_before_their_owner() {
        let dir = platform_owner_fixture();
        let runner = RealGitRunner;
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let root = capability::canonical_root(dir.path()).unwrap();
        let workspace = Workspace::load(root, &locator, &runner).unwrap();
        let owner = ReleasePackageId::new(Ecosystem::Npm, "@s/cli").unwrap();
        let version = Version::parse("0.1.0", VersionGrammar::SemVer).unwrap();
        let decision = ReleaseDecisionV1::new(vec![callisto_model::ReleaseDecisionEntry {
            package: owner.clone(),
            target_version: version.clone(),
            reasons: vec![callisto_model::ReleaseInclusionReason::ExplicitSelection],
        }])
        .unwrap();
        let source = capability::observe_source(&workspace, ExecutionTrustProfileV1::GitCommit).unwrap();
        let (intent, prepared) = derive::derive_release_intent_with_prepared(
            &workspace,
            &decision,
            ReleaseProfileId::production(),
            source,
            ExecutionTrustProfileV1::GitCommit,
            None,
        )
        .unwrap();

        let role_of = |operation: &callisto_model::ReleaseOperation| operation.id().role.clone();
        let platforms: Vec<_> = intent
            .operations
            .iter()
            .filter(|operation| {
                matches!(
                    role_of(operation),
                    callisto_model::ReleaseOperationRole::PlatformPublish { .. }
                )
            })
            .collect();
        assert_eq!(platforms.len(), 2);
        let publish = intent
            .operations
            .iter()
            .find(|operation| {
                matches!(
                    role_of(operation),
                    callisto_model::ReleaseOperationRole::RegistryPublish { .. }
                )
            })
            .unwrap();
        for platform in &platforms {
            assert_eq!(
                platform.id().package,
                owner,
                "a platform publish is authorized as its owner"
            );
            assert!(publish.prerequisites().contains(platform.id()));
            let PreparedOperation::RegistryPublish(prepared) = &prepared.operations[platform.id()] else {
                panic!("a platform publish must route to the registry provider");
            };
            assert!(prepared.by_directory);
            assert!(prepared.package_dir.starts_with("packages/cli/npm"));
            assert_eq!(prepared.npm_access, Some(callisto_model::NpmAccess::Public));
        }
        assert_eq!(
            intent
                .operations
                .iter()
                .filter(|operation| operation.id().role == callisto_model::ReleaseOperationRole::Tag)
                .count(),
            1,
            "only the owner is tagged"
        );
        assert_eq!(
            intent.operations.len(),
            4,
            "two platforms, the owner publish, the owner tag"
        );

        // One platform failing leaves its sibling runnable and the owner blocked.
        let mut state = crate::commands::release_test_support::pending_state(&intent);
        assert_eq!(
            crate::commands::release_execution::eligible_operations(&intent, &state),
            [platforms[0].id().clone(), platforms[1].id().clone()]
        );
        state
            .apply(
                platforms[0].id(),
                &crate::commands::release_test_support::attempt_event(),
            )
            .unwrap();
        state
            .apply(
                platforms[0].id(),
                &callisto_model::OperationEvent::Blocked {
                    reason: callisto_model::OperationBlockReason::IndeterminateAttempt,
                },
            )
            .unwrap();
        crate::commands::release_test_support::publish_operation(&mut state, platforms[1].id());
        let eligible = crate::commands::release_execution::eligible_operations(&intent, &state);
        assert!(eligible.is_empty(), "the owner must not publish: {eligible:?}");
    }

    #[test]
    fn fresh_validation_rejects_manifest_change() {
        let (dir, runner) = fixture();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let intent = build_release_intent(
            dir.path(),
            &locator,
            &runner,
            &decision(),
            ReleaseProfileId::production(),
            ExecutionTrustProfileV1::GitCommit,
        )
        .unwrap();
        validate_release_intent(dir.path(), &locator, &runner, intent.clone()).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"release-fixture\"\nversion = \"1.2.4\"\nedition = \"2021\"\n",
        )
        .unwrap();
        let error = validate_release_intent(dir.path(), &locator, &runner, intent)
            .expect_err("a dirty checkout cannot produce Git commit trust evidence");
        assert!(matches!(error, GraphError::Vcs(_)));
    }

    #[test]
    fn prepared_capability_retains_exact_tag_and_registry_inputs() {
        let (dir, runner) = fixture();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let intent = build_release_intent(
            dir.path(),
            &locator,
            &runner,
            &decision(),
            ReleaseProfileId::production(),
            ExecutionTrustProfileV1::GitCommit,
        )
        .unwrap();
        let validated = validate_release_intent(dir.path(), &locator, &runner, intent).unwrap();
        let inputs = validated.prepared();
        assert!(inputs.root.is_absolute());
        assert!(matches!(&inputs.source, SourceIdentity::GitCommit { .. }));

        let tag = inputs
            .operations
            .values()
            .find_map(|operation| match operation {
                PreparedOperation::Tag(tag) => Some((&tag.name, &tag.target, &tag.annotation)),
                _ => None,
            })
            .expect("the tag operation must retain its render, target, and annotation policy");
        assert_eq!(tag.0.as_str(), "release-fixture@1.2.3");
        assert_eq!(tag.2, "Release release-fixture@1.2.3");
        assert_eq!(tag.1.as_str().len(), 40);

        let registry = inputs
            .operations
            .values()
            .find_map(|operation| match operation {
                PreparedOperation::RegistryPublish(publish) => Some((
                    &publish.package_dir,
                    &publish.package_name,
                    &publish.version,
                    &publish.registry,
                    &publish.npm_access,
                )),
                _ => None,
            })
            .expect("the publish operation must retain its exact routing input");
        assert_eq!(registry.0, &std::path::PathBuf::new());
        assert_eq!(registry.1, "release-fixture");
        assert_eq!(registry.2.render(), "1.2.3");
        assert_eq!(registry.3.key.as_str(), "cratesIo");
        assert!(registry.3.endpoint.is_none());
        assert!(registry.4.is_none());
    }

    #[test]
    fn prepared_tag_adapter_accepts_only_the_exact_intended_target() {
        let (dir, _) = fixture();
        let state_dir = tempfile::tempdir().unwrap();
        let runner = StubbedRemote {
            ls_remote: state_dir.path().join("ls-remote"),
        };
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let intent = build_release_intent(
            dir.path(),
            &locator,
            &runner,
            &decision(),
            ReleaseProfileId::production(),
            ExecutionTrustProfileV1::GitCommit,
        )
        .unwrap();
        let validated = validate_release_intent(dir.path(), &locator, &runner, intent).unwrap();
        let tag_id = ReleaseProviderSet::intent(&validated)
            .operations
            .iter()
            .find(|operation| matches!(operation.id().role, callisto_model::ReleaseOperationRole::Tag))
            .expect("tag operation")
            .id()
            .clone();
        let tag_name = "release-fixture@1.2.3";
        assert!(std::process::Command::new("git")
            .args([
                "-c",
                "tag.gpgSign=false",
                "tag",
                "-a",
                tag_name,
                "HEAD",
                "-m",
                "Release release-fixture@1.2.3"
            ])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
        // The remote, not the local ref, is what satisfies the operation.
        let object = git_stdout(dir.path(), &["rev-parse", &format!("refs/tags/{tag_name}")]);
        let commit = git_stdout(dir.path(), &["rev-parse", &format!("refs/tags/{tag_name}^{{commit}}")]);
        std::fs::write(
            &runner.ls_remote,
            format!("{object}\trefs/tags/{tag_name}\n{commit}\trefs/tags/{tag_name}^{{}}\n"),
        )
        .unwrap();
        assert!(matches!(
            validated.preflight(&tag_id, None).unwrap(),
            ReleasePreflight::AlreadySatisfied { .. }
        ));
    }

    #[test]
    fn prepared_tag_adapter_rejects_conflicting_existing_target() {
        let (dir, _) = fixture();
        let state_dir = tempfile::tempdir().unwrap();
        // An empty answer: the remote has no such tag, so the conflicting local one decides.
        let runner = StubbedRemote {
            ls_remote: state_dir.path().join("ls-remote"),
        };
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let intent = build_release_intent(
            dir.path(),
            &locator,
            &runner,
            &decision(),
            ReleaseProfileId::production(),
            ExecutionTrustProfileV1::GitCommit,
        )
        .unwrap();
        let validated = validate_release_intent(dir.path(), &locator, &runner, intent).unwrap();
        let tag_id = ReleaseProviderSet::intent(&validated)
            .operations
            .iter()
            .find(|operation| matches!(operation.id().role, callisto_model::ReleaseOperationRole::Tag))
            .expect("tag operation")
            .id()
            .clone();
        assert!(std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-qm", "other"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .args([
                "-c",
                "tag.gpgSign=false",
                "tag",
                "-a",
                "release-fixture@1.2.3",
                "HEAD",
                "-m",
                "wrong",
            ])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
        assert!(matches!(
            validated.preflight(&tag_id, None),
            Err(GraphError::ReleaseRemoteConflict {
                conflict: RemoteConflict::TagTargetDiffers
            })
        ));
    }
}
