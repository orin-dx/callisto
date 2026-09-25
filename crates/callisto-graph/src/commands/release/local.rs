//! The local `callisto release` route: select every unreleased package and
//! derive its intent, for both the preview and the run.

use std::path::Path;

use callisto_model::{CommandRunner, ExecutionTrustProfileV1, ReleaseIntentV1, ReleasePackageId, SourceIdentity};

use crate::{DependencyResolver, GraphError, ProjectLocator, Workspace};

use super::binding::optional_git_remote;
use super::capability::{canonical_root, observe_source, ReleaseCheckout};
use super::derive::{derive_release_intent, ArtifactBuildPolicy, GitRemoteRequirement};
use super::StaleReason;

/// How a local release observes its source commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalReleaseSource {
    /// `--dry-run`: HEAD's commit, whatever the worktree state.
    Preview,
    /// A real run: HEAD's commit from a clean worktree, on any branch.
    Trusted,
}

/// Why a workspace must release through the CI plan/build/execute route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiReleaseRoute {
    /// The workspace declares `[[release.artifact]]` slots.
    ArtifactSlots,
    /// A package ships napi or maturin platform builds.
    PlatformPackage { package: String },
}

impl std::fmt::Display for CiReleaseRoute {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CiReleaseRoute::ArtifactSlots => formatter.write_str("the workspace declares [[release.artifact]] slots"),
            CiReleaseRoute::PlatformPackage { package } => {
                write!(formatter, "package `{package}` ships napi/maturin platform builds")
            }
        }
    }
}

/// The reason this workspace cannot release locally, if any.
pub fn ci_release_route<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
) -> Result<Option<CiReleaseRoute>, GraphError> {
    if workspace
        .config
        .product_release
        .as_ref()
        .is_some_and(|release| !release.artifacts.is_empty())
    {
        return Ok(Some(CiReleaseRoute::ArtifactSlots));
    }
    if let Some(package) = workspace.graph.packages().find(|package| {
        super::derive::is_platform_package(package) || workspace.identity.platforms_of(&package.id).next().is_some()
    }) {
        return Ok(Some(CiReleaseRoute::PlatformPackage {
            package: package.id.display_name(),
        }));
    }
    let matrix = crate::commands::matrix(workspace, &crate::commands::MatrixOptions::default())?;
    Ok(matrix
        .platform_targets
        .keys()
        .next()
        .map(|package| CiReleaseRoute::PlatformPackage {
            package: package.clone(),
        }))
}

/// A derived local release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalReleasePlan {
    pub intent: ReleaseIntentV1,
    /// A preview only: `origin` has no push URL, so tags carry no remote and a run would refuse.
    pub tags_unbound: bool,
}

/// Derives the intent for every unreleased package (or exactly `selections`).
///
/// This is the one plan derivation behind both `callisto release --dry-run`
/// and `callisto release`; `None` means nothing is unreleased.
pub fn plan_local_release<L: ProjectLocator, R: CommandRunner>(
    root: &Path,
    locator: &L,
    runner: &R,
    selections: &[ReleasePackageId],
    mode: LocalReleaseSource,
) -> Result<Option<LocalReleasePlan>, GraphError> {
    let root = canonical_root(root)?;
    let workspace = Workspace::load(root, locator, runner)?;
    plan_workspace_release(&workspace, selections, mode)
}

/// [`plan_local_release`] over an already-loaded workspace, e.g. one built from an in-memory config.
pub fn plan_workspace_release<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    selections: &[ReleasePackageId],
    mode: LocalReleaseSource,
) -> Result<Option<LocalReleasePlan>, GraphError> {
    let Some(decision) = crate::commands::derive_unreleased_decision(workspace, selections)? else {
        return Ok(None);
    };
    let profile = ExecutionTrustProfileV1::GitCommit;
    let source = match mode {
        LocalReleaseSource::Preview => SourceIdentity::GitCommit {
            sha: workspace.git_access().head_sha()?,
        },
        LocalReleaseSource::Trusted => observe_source(workspace, profile, ReleaseCheckout::AnyHead)?,
    };
    // A local run is its own orchestration: slots bind to the source commit.
    let artifact_policy = match (&workspace.config.product_release, &source) {
        (Some(release), SourceIdentity::GitCommit { sha }) if !release.artifacts.is_empty() => {
            release.forge_repository.clone().map(|repository| ArtifactBuildPolicy {
                repository,
                workflow_path: callisto_model::RELEASE_COORDINATOR_WORKFLOW_PATH.to_owned(),
                workflow_commit: sha.clone(),
            })
        }
        _ => None,
    };
    let remote = match mode {
        LocalReleaseSource::Preview => GitRemoteRequirement::OptionalForPreview,
        LocalReleaseSource::Trusted => GitRemoteRequirement::Required,
    };
    let intent = derive_release_intent(
        workspace,
        &decision,
        source.clone(),
        profile,
        artifact_policy.as_ref(),
        remote,
    )?;
    let tags_unbound = mode == LocalReleaseSource::Preview
        && intent
            .operations
            .iter()
            .any(|operation| operation.id().role == callisto_model::ReleaseOperationRole::Tag)
        && optional_git_remote(&workspace.root, workspace.runner)?.is_none();
    if mode == LocalReleaseSource::Trusted && observe_source(workspace, profile, ReleaseCheckout::AnyHead)? != source {
        return Err(GraphError::ReleaseIntentStale {
            reason: StaleReason::source_identity_changed(),
        });
    }
    Ok(Some(LocalReleasePlan { intent, tags_unbound }))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use callisto_model::{Ecosystem, ReleaseInclusionReason, ReleasePackageId};

    use super::*;
    use crate::commands::release::tests::{fixture, RealGitRunner};
    use crate::error::{ReleasePreconditionRequirement, ReleaseSelectionInvalidReason};

    fn git(root: &Path, args: &[&str]) {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }

    fn plan(
        root: &Path,
        selections: &[ReleasePackageId],
        mode: LocalReleaseSource,
    ) -> Result<Option<ReleaseIntentV1>, GraphError> {
        plan_local_release(
            root,
            &crate::IgnoreWalkLocator::new(root),
            &RealGitRunner,
            selections,
            mode,
        )
        .map(|plan| plan.map(|plan| plan.intent))
    }

    #[test]
    fn preview_without_origin_leaves_tags_unbound_and_the_run_refuses() {
        let (dir, _) = fixture();
        git(dir.path(), &["remote", "remove", "origin"]);
        let preview = plan_local_release(
            dir.path(),
            &crate::IgnoreWalkLocator::new(dir.path()),
            &RealGitRunner,
            &[],
            LocalReleaseSource::Preview,
        )
        .unwrap()
        .expect("unreleased");
        assert!(preview.tags_unbound);
        assert!(preview
            .intent
            .operations
            .iter()
            .any(|operation| operation.id().role == callisto_model::ReleaseOperationRole::Tag));
        assert!(matches!(
            plan(dir.path(), &[], LocalReleaseSource::Trusted).unwrap_err(),
            GraphError::UnsafeGitRemote { .. }
        ));
    }

    #[test]
    fn preview_with_origin_binds_tags() {
        let (dir, _) = fixture();
        let preview = plan_local_release(
            dir.path(),
            &crate::IgnoreWalkLocator::new(dir.path()),
            &RealGitRunner,
            &[],
            LocalReleaseSource::Preview,
        )
        .unwrap()
        .unwrap();
        assert!(!preview.tags_unbound);
    }

    fn fixture_id() -> ReleasePackageId {
        ReleasePackageId::new(Ecosystem::Cargo, "release-fixture").unwrap()
    }

    fn head(root: &Path) -> String {
        RealGitRunner
            .run("git", &["rev-parse", "HEAD"], root)
            .unwrap()
            .stdout
            .trim()
            .to_owned()
    }

    #[test]
    fn untagged_current_version_is_selected_as_unreleased() {
        let (dir, _) = fixture();
        let intent = plan(dir.path(), &[], LocalReleaseSource::Preview)
            .unwrap()
            .expect("unreleased");
        assert_eq!(intent.decision.entries.len(), 1);
        let entry = &intent.decision.entries[0];
        assert_eq!(entry.package, fixture_id());
        assert_eq!(entry.target_version.render(), "1.2.3");
        assert_eq!(entry.reasons, vec![ReleaseInclusionReason::UnreleasedVersion]);
    }

    #[test]
    fn tagged_current_version_means_nothing_to_release() {
        let (dir, _) = fixture();
        git(dir.path(), &["-c", "tag.gpgSign=false", "tag", "release-fixture@1.2.3"]);
        assert!(plan(dir.path(), &[], LocalReleaseSource::Preview).unwrap().is_none());
        assert!(plan(dir.path(), &[], LocalReleaseSource::Trusted).unwrap().is_none());
    }

    #[test]
    fn trusted_run_accepts_a_branch_and_records_head() {
        let (dir, _) = fixture();
        git(dir.path(), &["checkout", "-q", "-b", "main"]);
        let intent = plan(dir.path(), &[], LocalReleaseSource::Trusted)
            .unwrap()
            .expect("unreleased");
        assert_eq!(
            intent.snapshot.source,
            SourceIdentity::GitCommit {
                sha: callisto_model::CommitSha::parse(&head(dir.path())).unwrap()
            }
        );
        let validated = crate::commands::validate_local_release_intent(
            dir.path(),
            &crate::IgnoreWalkLocator::new(dir.path()),
            &RealGitRunner,
            intent.clone(),
        );
        assert!(validated.is_ok(), "{validated:?}");
        let error = crate::commands::validate_release_intent(
            dir.path(),
            &crate::IgnoreWalkLocator::new(dir.path()),
            &RealGitRunner,
            intent,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            GraphError::ReleasePreconditionUnmet {
                requirement: ReleasePreconditionRequirement::DetachedHead
            }
        ));
    }

    #[test]
    fn preview_and_trusted_derive_the_same_intent() {
        let (dir, _) = fixture();
        let preview = plan(dir.path(), &[], LocalReleaseSource::Preview).unwrap();
        let trusted = plan(dir.path(), &[], LocalReleaseSource::Trusted).unwrap();
        assert_eq!(preview, trusted);
    }

    #[test]
    fn dirty_worktree_blocks_a_trusted_run_but_not_a_preview() {
        for dirt in ["tracked", "untracked"] {
            let (dir, _) = fixture();
            match dirt {
                "tracked" => std::fs::write(
                    dir.path().join("Cargo.toml"),
                    "[package]\nname = \"release-fixture\"\nversion = \"1.2.3\"\nedition = \"2021\"\n# edit\n",
                )
                .unwrap(),
                _ => std::fs::write(dir.path().join("scratch.txt"), "x").unwrap(),
            }
            let error = plan(dir.path(), &[], LocalReleaseSource::Trusted).unwrap_err();
            assert!(error.to_string().contains("clean worktree"), "{dirt}: {error}");
            assert!(plan(dir.path(), &[], LocalReleaseSource::Preview).unwrap().is_some());
        }
    }

    #[test]
    fn ignored_files_do_not_block_a_trusted_run() {
        let (dir, _) = fixture();
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        git(dir.path(), &["add", ".gitignore"]);
        git(dir.path(), &["commit", "-q", "-m", "ignore"]);
        std::fs::create_dir(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/out"), "x").unwrap();
        assert!(plan(dir.path(), &[], LocalReleaseSource::Trusted).unwrap().is_some());
    }

    #[test]
    fn package_selection_must_name_an_unreleased_package() {
        let (dir, _) = fixture();
        let selected = plan(dir.path(), &[fixture_id()], LocalReleaseSource::Preview)
            .unwrap()
            .unwrap();
        assert_eq!(selected.decision.entries[0].package, fixture_id());

        let unknown = ReleasePackageId::new(Ecosystem::Cargo, "nope").unwrap();
        assert!(matches!(
            plan(dir.path(), &[unknown], LocalReleaseSource::Preview).unwrap_err(),
            GraphError::UnknownPackage { .. }
        ));

        git(dir.path(), &["-c", "tag.gpgSign=false", "tag", "release-fixture@1.2.3"]);
        assert!(matches!(
            plan(dir.path(), &[fixture_id()], LocalReleaseSource::Preview).unwrap_err(),
            GraphError::ReleaseSelectionInvalid {
                reason: ReleaseSelectionInvalidReason::NotARelease,
                ..
            }
        ));
    }

    fn grouped_repo(kind: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"a\", \"b\", \"c\"]\n",
        )
        .unwrap();
        for name in ["a", "b", "c"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(
                root.join(name).join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n"),
            )
            .unwrap();
        }
        std::fs::write(
            root.join("callisto.toml"),
            format!("[[{kind}-group]]\nname = \"g\"\nmembers = [\"a\", \"b\"]\n\n[[package]]\nmatch = \"*\"\npublish-to = [\"crates-io\"]\n"),
        )
        .unwrap();
        for args in [
            ["init", "-q"].as_slice(),
            ["config", "user.email", "test@example.com"].as_slice(),
            ["config", "user.name", "Test"].as_slice(),
            ["config", "commit.gpgsign", "false"].as_slice(),
            ["remote", "add", "origin", "https://github.com/example/grouped.git"].as_slice(),
            ["add", "."].as_slice(),
            ["commit", "-q", "-m", "fixture"].as_slice(),
        ] {
            git(root, args);
        }
        dir
    }

    /// M1: `--package` keeps every fixed- or linked-group member, as the CI route does.
    #[test]
    fn package_selection_expands_fixed_and_linked_groups() {
        for kind in ["fixed", "linked"] {
            let dir = grouped_repo(kind);
            let id = |name| ReleasePackageId::new(Ecosystem::Cargo, name).unwrap();
            let intent = plan(dir.path(), &[id("a")], LocalReleaseSource::Preview)
                .unwrap()
                .unwrap();
            let packages: Vec<_> = intent
                .decision
                .entries
                .iter()
                .map(|entry| entry.package.clone())
                .collect();
            assert_eq!(packages, [id("a"), id("b")], "{kind}");
        }
    }

    fn route(root: &Path) -> Option<CiReleaseRoute> {
        let locator = crate::IgnoreWalkLocator::new(root);
        let workspace = Workspace::load(root.to_path_buf(), &locator, &RealGitRunner).unwrap();
        ci_release_route(&workspace).unwrap()
    }

    #[test]
    fn artifact_slots_and_platform_packages_require_the_ci_route() {
        let (dir, _) = fixture();
        assert_eq!(route(dir.path()), None);

        let config = std::fs::read_to_string(dir.path().join("callisto.toml")).unwrap();
        std::fs::write(
            dir.path().join("callisto.toml"),
            format!("{config}[release]\nproduct-package = \"cargo/release-fixture\"\nforge-repository = \"example/release-fixture\"\n\n[[release.artifact]]\npackage = \"cargo/release-fixture\"\ntarget = \"x86_64-unknown-linux-gnu\"\nasset-name = \"fixture.tar.gz\"\n"),
        )
        .unwrap();
        assert_eq!(route(dir.path()), Some(CiReleaseRoute::ArtifactSlots));

        let napi = tempfile::tempdir().unwrap();
        std::fs::write(
            napi.path().join("package.json"),
            r#"{"name":"addon","version":"1.0.0","napi":{"targets":["aarch64-apple-darwin"]}}"#,
        )
        .unwrap();
        git(napi.path(), &["init", "-q"]);
        assert!(matches!(
            route(napi.path()),
            Some(CiReleaseRoute::PlatformPackage { .. })
        ));
    }
}
