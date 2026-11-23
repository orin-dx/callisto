//! Fresh, graph-owned authorization for durable release intents.
//!
//! This module intentionally takes a root, locator, and runner rather than a
//! `Workspace`. A workspace caches parsed manifests and config for a normal
//! command invocation; accepting one here would make an already observed
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
mod local;
mod notes;
mod pending;
pub(crate) mod provider;
mod version_commit;

use super::release_artifacts;

pub(crate) use binding::optional_git_remote;
pub(crate) use capability::canonical_root;
pub use capability::{
    build_release_intent, validate_local_release_intent, validate_release_intent, ValidatedReleaseIntent,
};
#[cfg(test)]
pub(crate) use derive::canonical_operation_order;
pub(crate) use derive::is_platform_package;
pub use derive::ArtifactBuildPolicy;
pub use local::{
    ci_release_route, origin_tag_probe, plan_loaded_release, plan_workspace_release, plan_workspace_release_offline,
    CiReleaseRoute, LocalReleasePlan, LocalReleaseSource, OriginTagProbe,
};
pub use pending::{pending_releases, PendingRelease};
pub(crate) use provider::policy::{programs, timeouts};
pub use provider::{ReleasePreflight, ReleaseProviderSet};

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use callisto_model::{
        CommandError, CommandOutput, CommandRunner, Ecosystem, ExecutionTrustProfileV1, ReleaseDecisionV1,
        ReleasePackageId, SourceIdentity, Version, VersionGrammar,
    };

    use super::provider::PreparedOperation;
    use super::*;
    use crate::error::RemoteConflict;
    use crate::{DependencyResolver, GraphError, Workspace};

    pub(crate) struct RealGitRunner;
    impl CommandRunner for RealGitRunner {
        fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
            // A registry lookup never reaches the network from a test: every crate is unpublished.
            if program == "cargo" && args.contains(&"info") {
                let spec = args
                    .iter()
                    .skip_while(|arg| **arg != "info")
                    .nth(1)
                    .copied()
                    .unwrap_or_default();
                return Ok(CommandOutput {
                    exit_code: Some(101),
                    stdout: String::new(),
                    stderr: format!("error: could not find `{spec}` in registry `crates-io`\n"),
                });
            }
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

        fn run_with_stdin(
            &self,
            program: &str,
            args: &[&str],
            cwd: &Path,
            stdin: &[u8],
        ) -> Result<CommandOutput, CommandError> {
            callisto_fixtures::git::GitRunner.run_with_stdin(program, args, cwd, stdin)
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

        fn run_with_stdin(
            &self,
            program: &str,
            args: &[&str],
            cwd: &Path,
            stdin: &[u8],
        ) -> Result<CommandOutput, CommandError> {
            RealGitRunner.run_with_stdin(program, args, cwd, stdin)
        }
    }

    fn git_stdout(root: &Path, args: &[&str]) -> String {
        let output = callisto_model::vcs::GitStdout::of(RealGitRunner.run("git", args, root).unwrap()).unwrap();
        output.line().to_owned()
    }

    pub(crate) fn fixture() -> (tempfile::TempDir, RealGitRunner) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"release-fixture\"\nversion = \"1.2.3\"\nedition = \"2021\"\ndescription = \"t\"\nlicense = \"MIT\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("callisto.toml"),
            "[[package]]\nmatch = \"cargo/release-fixture\"\npublish-to = [\"crates-io\"]\n",
        )
        .unwrap();
        callisto_fixtures::git::init_repo(dir.path());
        for args in [
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

    /// A dual-manifest package (one directory with a Cargo and an npm manifest) gets the same two
    /// `ReleasePackageId`s from the roster `derive_release_decision` computes and from the snapshot
    /// `build_release_intent` derives from it.
    #[test]
    fn dual_identity_package_release_ids_agree_between_decision_and_intent_derivation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"release-fixture\"\nversion = \"1.2.3\"\nedition = \"2021\"\ndescription = \"t\"\nlicense = \"MIT\"\n",
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
            "[[package]]\nmatch = \"cargo/release-fixture\"\npublish-to = []\n",
        )
        .unwrap();
        callisto_fixtures::git::init_repo(dir.path());
        for args in [
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
            "the Cargo and npm manifest must merge into one dual-manifest package"
        );

        let plan = crate::VersionPlan {
            bumps: vec![crate::PlannedBump {
                package: package_id,
                from: Version::parse("1.2.3", VersionGrammar::SemVer).unwrap(),
                to: Version::parse("1.3.0", VersionGrammar::SemVer).unwrap(),
                severity: callisto_model::Severity::Minor,
                governed_by: None,
                trigger: Some(callisto_model::BumpReason::Changeset { changesets: vec![] }),
                writes: vec![],
                severity_sources: Default::default(),
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
            ExecutionTrustProfileV1::GitCommit,
            None,
        )
        .unwrap()
        .0;
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

    /// A dual-manifest package's npm identity is its package.json `name`, not the crate name.
    #[test]
    fn case_d_npm_release_identity_uses_the_package_json_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"addon\"\nversion = \"1.2.3\"\nedition = \"2021\"\ndescription = \"t\"\nlicense = \"MIT\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"name":"@s/addon","version":"1.2.3"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("callisto.toml"),
            "[[package]]\nmatch = \"cargo/addon\"\npublish-to = []\n",
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
        callisto_fixtures::git::init_repo(dir.path());
        for args in [
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

    /// Each attached platform package is one `PlatformPublish`
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
        let source = capability::observe_source(
            &workspace,
            ExecutionTrustProfileV1::GitCommit,
            super::capability::ReleaseCheckout::Detached,
        )
        .unwrap();
        let (intent, prepared) = derive::derive_release_intent_with_prepared(
            &workspace,
            &decision,
            source,
            ExecutionTrustProfileV1::GitCommit,
            None,
            &mut Vec::new(),
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

    /// Git is real; a pack writes the tarball its package.json names, and `tar` reads that package.json back.
    struct PackFake {
        packed: std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, String>>,
        packs: std::sync::Mutex<Vec<String>>,
        publishes: std::sync::Mutex<Vec<Vec<String>>>,
    }

    impl CommandRunner for PackFake {
        fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
            let ok = |stdout: String| {
                Ok(CommandOutput {
                    exit_code: Some(0),
                    stdout,
                    stderr: String::new(),
                })
            };
            if program == "tar" {
                let tarball = Path::new(args[1]);
                return ok(self.packed.lock().unwrap()[tarball.parent().unwrap()].clone());
            }
            if program == "npm" && args.first() == Some(&"publish") {
                self.publishes
                    .lock()
                    .unwrap()
                    .push(args.iter().map(|arg| (*arg).to_owned()).collect());
                return ok(String::new());
            }
            let Some(at) = args.iter().position(|arg| *arg == "--pack-destination") else {
                return RealGitRunner.run(program, args, cwd);
            };
            let destination = std::path::PathBuf::from(args[at + 1]);
            let json = std::fs::read_to_string(cwd.join("package.json")).unwrap();
            let manifest: serde_json::Value = serde_json::from_str(&json).unwrap();
            let name = manifest["name"]
                .as_str()
                .unwrap()
                .trim_start_matches('@')
                .replace('/', "-");
            std::fs::write(
                destination.join(format!("{name}-{}.tgz", manifest["version"].as_str().unwrap())),
                b"tarball",
            )
            .unwrap();
            self.packs.lock().unwrap().push(program.to_owned());
            self.packed.lock().unwrap().insert(destination, json);
            ok(String::new())
        }

        fn run_with_timeout_env_filtered(
            &self,
            program: &str,
            args: &[&str],
            cwd: &Path,
            _timeout: std::time::Duration,
            _removed: &[&str],
        ) -> Result<CommandOutput, CommandError> {
            self.run(program, args, cwd)
        }
    }

    /// The pack job packs each npm publish of the plan, an owner and its platform packages each once, and nothing else.
    #[test]
    fn packing_covers_every_npm_publish_of_the_plan_and_no_other_publish() {
        let dir = platform_owner_fixture();
        let runner = PackFake {
            packed: Default::default(),
            packs: Default::default(),
            publishes: Default::default(),
        };
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let owner = ReleasePackageId::new(Ecosystem::Npm, "@s/cli").unwrap();
        let decision = ReleaseDecisionV1::new(vec![callisto_model::ReleaseDecisionEntry {
            package: owner,
            target_version: Version::parse("0.1.0", VersionGrammar::SemVer).unwrap(),
            reasons: vec![callisto_model::ReleaseInclusionReason::ExplicitSelection],
        }])
        .unwrap();
        let intent = build_release_intent(
            dir.path(),
            &locator,
            &runner,
            &decision,
            ExecutionTrustProfileV1::GitCommit,
            None,
        )
        .unwrap()
        .0;
        let validated = validate_release_intent(dir.path(), &locator, &runner, intent).unwrap();
        let out = tempfile::tempdir().unwrap();

        let packed = validated.pack_npm_tarballs(out.path()).unwrap();
        assert_eq!(packed, ["@s/cli", "@s/cli-darwin-arm64", "@s/cli-linux-x64-gnu"]);
        let mut files: Vec<String> = std::fs::read_dir(out.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        let mut expected: Vec<String> = ["@s/cli", "@s/cli-darwin-arm64", "@s/cli-linux-x64-gnu"]
            .map(|name| {
                callisto_model::npm_tarball_filename(name, &Version::parse("0.1.0", VersionGrammar::SemVer).unwrap())
            })
            .to_vec();
        expected.sort();
        assert_eq!(files, expected);
        let mut programs = runner.packs.lock().unwrap().clone();
        programs.sort();
        assert_eq!(
            programs,
            ["npm", "npm", "pnpm"],
            "the owner packs with pnpm, platform packages with npm"
        );
    }

    /// `@acme/lib` and `acme-lib` flatten to one name in npm's own file naming. Each is staged under its own name, so
    /// both survive the pack, both are sealed, and the sealed publish uploads each one's own file.
    #[test]
    fn packages_whose_names_flatten_alike_are_packed_sealed_and_published_separately() {
        let dir = tempfile::tempdir().unwrap();
        let write = |rel: &str, body: &str| {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        write(
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        );
        write(
            "packages/scoped/package.json",
            r#"{"name":"@acme/lib","version":"1.0.0"}"#,
        );
        write(
            "packages/plain/package.json",
            r#"{"name":"acme-lib","version":"1.0.0"}"#,
        );
        write(
            "callisto.toml",
            "[[package]]\nmatch = \"npm/@acme/lib\"\npublish-to = [\"npm\"]\n\n[[package]]\nmatch = \"npm/acme-lib\"\npublish-to = [\"npm\"]\n",
        );
        callisto_fixtures::git::init_repo(dir.path());
        for args in [
            ["remote", "add", "origin", "https://github.com/example/acme.git"].as_slice(),
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
        let runner = PackFake {
            packed: Default::default(),
            packs: Default::default(),
            publishes: Default::default(),
        };
        let version = Version::parse("1.0.0", VersionGrammar::SemVer).unwrap();
        let decision = ReleaseDecisionV1::new(
            ["@acme/lib", "acme-lib"]
                .map(|name| callisto_model::ReleaseDecisionEntry {
                    package: ReleasePackageId::new(Ecosystem::Npm, name).unwrap(),
                    target_version: version.clone(),
                    reasons: vec![callisto_model::ReleaseInclusionReason::ExplicitSelection],
                })
                .to_vec(),
        )
        .unwrap();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let intent = build_release_intent(
            dir.path(),
            &locator,
            &runner,
            &decision,
            ExecutionTrustProfileV1::GitCommit,
            None,
        )
        .unwrap()
        .0;
        let validated = validate_release_intent(dir.path(), &locator, &runner, intent.clone()).unwrap();
        let out = tempfile::tempdir().unwrap();

        assert_eq!(
            validated.pack_npm_tarballs(out.path()).unwrap(),
            ["@acme/lib", "acme-lib"]
        );
        assert_eq!(
            std::fs::read_dir(out.path()).unwrap().count(),
            2,
            "neither tarball overwrote the other"
        );

        let sets = crate::commands::seal_npm_tarballs(&intent, out.path()).unwrap();
        assert_eq!(sets.len(), 2);
        let manifest = callisto_model::ArtifactManifestV1::with_distributions(&intent, vec![], sets).unwrap();
        let verified = crate::commands::release_artifacts::VerifiedArtifactManifest::for_tests(
            &manifest,
            out.path().canonicalize().unwrap(),
        );
        let context = super::provider::ProviderContext::new(&validated.prepared().root, &runner, None)
            .with_route(super::provider::PublishRoute::Ci);
        for (id, operation) in &validated.prepared().operations {
            let PreparedOperation::RegistryPublish(operation) = operation else {
                continue;
            };
            super::provider::registry::adapter_for(Ecosystem::Npm)
                .unwrap()
                .publish(&context, id, operation, Some(&verified))
                .unwrap();
        }
        let published = runner.publishes.lock().unwrap().clone();
        let mut tarballs: Vec<&String> = published.iter().map(|args| &args[1]).collect();
        tarballs.sort();
        tarballs.dedup();
        assert_eq!(tarballs.len(), 2, "each package publishes its own file: {published:?}");
        for tarball in tarballs {
            assert!(std::path::Path::new(tarball).is_file());
        }
    }

    /// Git is real; every other command is recorded and succeeds, so the `cargo package` calls can be read back.
    struct CargoPackageRecorder {
        calls: std::sync::Mutex<Vec<(std::path::PathBuf, Vec<String>)>>,
    }

    impl CommandRunner for CargoPackageRecorder {
        fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
            if program == "cargo" && args.first() == Some(&"package") {
                self.calls.lock().unwrap().push((
                    cwd.canonicalize().unwrap(),
                    args.iter().map(|arg| (*arg).to_owned()).collect(),
                ));
                return Ok(CommandOutput {
                    exit_code: Some(0),
                    stdout: String::new(),
                    stderr: String::new(),
                });
            }
            RealGitRunner.run(program, args, cwd)
        }

        fn run_with_timeout_env_filtered(
            &self,
            program: &str,
            args: &[&str],
            cwd: &Path,
            _timeout: std::time::Duration,
            _removed: &[&str],
        ) -> Result<CommandOutput, CommandError> {
            self.run(program, args, cwd)
        }
    }

    /// Packages the crates of a fixture whose manifests are `files` (crate name, manifest path) with the real
    /// derivation of the plan, and returns each `cargo package` call: where it ran and its arguments.
    fn package_calls(
        workspaces: &[(&str, &str)],
        crates: &[(&str, &str)],
        npm: &[&str],
    ) -> Vec<(std::path::PathBuf, Vec<String>)> {
        let dir = tempfile::tempdir().unwrap();
        let write = |rel: &str, body: &str| {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        for (path, members) in workspaces {
            write(path, &format!("[workspace]\nmembers = [{members}]\nresolver = \"2\"\n"));
        }
        let mut config = String::new();
        for (name, path) in crates {
            write(
                path,
                &format!(
                    "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\ndescription = \"t\"\nlicense = \"MIT\"\n"
                ),
            );
            write(&path.replace("Cargo.toml", "src/lib.rs"), "\n");
            config.push_str(&format!(
                "[[package]]\nmatch = \"cargo/{name}\"\npublish-to = [\"crates-io\"]\n\n"
            ));
        }
        for name in npm {
            write(
                &format!("{name}/package.json"),
                &format!(r#"{{"name":"{name}","version":"0.1.0"}}"#),
            );
            config.push_str(&format!(
                "[[package]]\nmatch = \"npm/{name}\"\npublish-to = [\"npm\"]\n\n"
            ));
        }
        write("callisto.toml", &config);
        callisto_fixtures::git::init_repo(dir.path());
        for args in [
            ["remote", "add", "origin", "https://github.com/example/crates.git"].as_slice(),
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
        let runner = CargoPackageRecorder {
            calls: Default::default(),
        };
        let entries = |ecosystem, names: Vec<&str>| {
            names
                .into_iter()
                .map(move |name| callisto_model::ReleaseDecisionEntry {
                    package: ReleasePackageId::new(ecosystem, name).unwrap(),
                    target_version: Version::parse("0.1.0", VersionGrammar::SemVer).unwrap(),
                    reasons: vec![callisto_model::ReleaseInclusionReason::ExplicitSelection],
                })
                .collect::<Vec<_>>()
        };
        let mut all = entries(Ecosystem::Cargo, crates.iter().map(|(name, _)| *name).collect());
        all.extend(entries(Ecosystem::Npm, npm.to_vec()));
        let decision = ReleaseDecisionV1::new(all).unwrap();
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let intent = build_release_intent(
            dir.path(),
            &locator,
            &runner,
            &decision,
            ExecutionTrustProfileV1::GitCommit,
            None,
        )
        .unwrap()
        .0;
        let validated = validate_release_intent(dir.path(), &locator, &runner, intent).unwrap();
        validated.package_cargo_crates().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let calls = runner.calls.lock().unwrap().clone();
        calls
            .into_iter()
            .map(|(cwd, args)| (cwd.strip_prefix(&root).unwrap().to_path_buf(), args))
            .collect()
    }

    fn call(cwd: &str, crates: &[&str]) -> (std::path::PathBuf, Vec<String>) {
        let mut args = vec!["package".to_owned(), "--locked".to_owned()];
        for name in crates {
            args.extend(["-p".to_owned(), (*name).to_owned()]);
        }
        (std::path::PathBuf::from(cwd), args)
    }

    /// A layout with its Cargo workspace under `native/` and no root manifest packages from there, once, with every
    /// crate of the release in that one call.
    #[test]
    fn crates_of_a_nested_cargo_workspace_are_packaged_together_from_its_directory() {
        let calls = package_calls(
            &[("native/Cargo.toml", "\"crates/*\"")],
            &[
                ("alpha", "native/crates/alpha/Cargo.toml"),
                ("beta", "native/crates/beta/Cargo.toml"),
            ],
            &[],
        );
        assert_eq!(calls, [call("native", &["alpha", "beta"])]);
    }

    /// Only crate publishes are packaged: an npm package of the same plan is never handed to cargo.
    #[test]
    fn an_npm_package_in_the_plan_is_not_packaged_with_cargo() {
        let calls = package_calls(&[("Cargo.toml", "\"core\"")], &[("core", "core/Cargo.toml")], &["web"]);
        assert_eq!(calls, [call("", &["core"])]);
    }

    /// One call per owning workspace, each from its own directory; the common single-workspace layout is one call
    /// from the root.
    #[test]
    fn each_cargo_workspace_of_a_release_is_packaged_in_its_own_call_from_its_own_directory() {
        let calls = package_calls(
            &[
                ("native/Cargo.toml", "\"crates/*\""),
                ("tools/Cargo.toml", "\"crates/*\""),
            ],
            &[
                ("alpha", "native/crates/alpha/Cargo.toml"),
                ("beta", "native/crates/beta/Cargo.toml"),
                ("gamma", "tools/crates/gamma/Cargo.toml"),
            ],
            &[],
        );
        assert_eq!(calls, [call("native", &["alpha", "beta"]), call("tools", &["gamma"])]);

        let single = package_calls(
            &[("Cargo.toml", "\"a\", \"b\"")],
            &[("a", "a/Cargo.toml"), ("b", "b/Cargo.toml")],
            &[],
        );
        assert_eq!(single, [call("", &["a", "b"])]);
    }

    /// A plan that publishes only a crate has nothing to pack.
    #[test]
    fn packing_a_plan_with_no_npm_publish_runs_no_pack() {
        let (dir, _) = fixture();
        let runner = PackFake {
            packed: Default::default(),
            packs: Default::default(),
            publishes: Default::default(),
        };
        let locator = crate::IgnoreWalkLocator::new(dir.path());
        let intent = build_release_intent(
            dir.path(),
            &locator,
            &runner,
            &decision(),
            ExecutionTrustProfileV1::GitCommit,
            None,
        )
        .unwrap()
        .0;
        let validated = validate_release_intent(dir.path(), &locator, &runner, intent).unwrap();
        let out = tempfile::tempdir().unwrap();
        assert!(validated.pack_npm_tarballs(out.path()).unwrap().is_empty());
        assert!(runner.packs.lock().unwrap().is_empty());
        assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0);
    }

    /// A yarn workspace publishing `lib` to npm, with `yarnrc` as its `.yarnrc.yml`.
    fn yarn_registry_fixture(yarnrc: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let write = |rel: &str, body: &str| {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        write(
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        );
        write("yarn.lock", "");
        write(".yarnrc.yml", yarnrc);
        write("packages/lib/package.json", r#"{"name":"lib","version":"1.0.0"}"#);
        write(
            "callisto.toml",
            "[[package]]\nmatch = \"npm/lib\"\npublish-to = [\"npm\"]\n",
        );
        callisto_fixtures::git::init_repo(dir.path());
        for args in [
            ["remote", "add", "origin", "https://github.com/example/lib.git"].as_slice(),
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

    /// The refusal comes with the intent, before any effect, so a plan and a local release stop on it alike.
    #[test]
    fn an_intent_for_a_yarn_workspace_with_a_registry_only_yarn_reads_is_refused() {
        let decision = ReleaseDecisionV1::new(vec![callisto_model::ReleaseDecisionEntry {
            package: ReleasePackageId::new(Ecosystem::Npm, "lib").unwrap(),
            target_version: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            reasons: vec![callisto_model::ReleaseInclusionReason::ExplicitSelection],
        }])
        .unwrap();
        let build = |yarnrc: &str| {
            let dir = yarn_registry_fixture(yarnrc);
            let locator = crate::IgnoreWalkLocator::new(dir.path());
            build_release_intent(
                dir.path(),
                &locator,
                &RealGitRunner,
                &decision,
                ExecutionTrustProfileV1::GitCommit,
                None,
            )
        };
        assert!(matches!(
            build("npmPublishRegistry: \"https://r.example/\"\n"),
            Err(GraphError::UnsupportedRelease {
                feature: crate::error::UnsupportedReleaseFeature::NpmRegistryConfig { .. }
            })
        ));
        build("nodeLinker: node-modules\n").unwrap();
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
            ExecutionTrustProfileV1::GitCommit,
            None,
        )
        .unwrap()
        .0;
        validate_release_intent(dir.path(), &locator, &runner, intent.clone()).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"release-fixture\"\nversion = \"1.2.4\"\nedition = \"2021\"\ndescription = \"t\"\nlicense = \"MIT\"\n",
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
            ExecutionTrustProfileV1::GitCommit,
            None,
        )
        .unwrap()
        .0;
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
        assert_eq!(registry.3.key.as_str(), "crates-io");
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
            ExecutionTrustProfileV1::GitCommit,
            None,
        )
        .unwrap()
        .0;
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
            ExecutionTrustProfileV1::GitCommit,
            None,
        )
        .unwrap()
        .0;
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
