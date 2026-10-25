//! The local `callisto release` route: select every unreleased package and
//! derive its intent, for both the preview and the run.

use std::path::Path;

use callisto_model::{CommandRunner, ExecutionTrustProfileV1, ReleaseIntentV1, ReleasePackageId, SourceIdentity};

use crate::{DependencyResolver, GraphError, ProjectLocator, Workspace};

use super::binding::optional_git_remote;
use super::capability::{
    canonical_root, names_taken_by_other_projects, observe_source, published_packages, unsatisfied_packages,
    ReleaseCheckout,
};
use super::derive::{
    derive_release_intent, derive_release_intent_with_prepared, ArtifactBuildPolicy, GitRemoteRequirement,
};
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

/// What it takes to ask `origin` which of a package's last-release tags it holds.
///
/// Built from the loaded workspace without touching the network, so the one `git ls-remote` in [`Self::missing`] can
/// run beside the rest of the plan.
#[derive(Clone, Debug)]
pub struct OriginTagProbe {
    endpoint: String,
    cwd: std::path::PathBuf,
    tags: std::collections::BTreeSet<String>,
}

/// The probe for `workspace`'s last-release tags, or `None` when there is no `origin` push URL or nothing is tagged.
pub fn origin_tag_probe<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
) -> Result<Option<OriginTagProbe>, GraphError> {
    let Some(remote) = optional_git_remote(&workspace.root, workspace.runner)? else {
        return Ok(None);
    };
    let tags = workspace.tags()?;
    let names: std::collections::BTreeSet<String> = workspace
        .graph
        .packages()
        .filter_map(|package| tags.last_tag(&package.id))
        .map(|tag| tag.name.as_str().to_owned())
        .collect();
    Ok((!names.is_empty()).then(|| OriginTagProbe {
        endpoint: remote.endpoint.clone(),
        cwd: workspace.root.clone(),
        tags: names,
    }))
}

impl OriginTagProbe {
    /// The tags that name a package's last release here but are not on `origin`.
    ///
    /// A release that tagged locally and then failed to push leaves exactly this, and a later rerun on another commit
    /// treats the tagged version as finished, so the remote tag and the forge release never happen. Best effort: a
    /// remote that cannot be read reports nothing.
    pub fn missing<R: CommandRunner + ?Sized>(&self, runner: &R) -> Vec<String> {
        let references: Vec<String> = self.tags.iter().map(|name| format!("refs/tags/{name}")).collect();
        let mut args = vec!["ls-remote", self.endpoint.as_str()];
        args.extend(references.iter().map(String::as_str));
        let Ok(output) = runner.run_with_timeout(
            super::provider::policy::programs::GIT,
            &args,
            &self.cwd,
            super::provider::policy::timeouts::GIT_LS_REMOTE,
        ) else {
            return Vec::new();
        };
        if !output.success() {
            return Vec::new();
        }
        let on_remote: std::collections::BTreeSet<&str> = output
            .stdout
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .filter_map(|(_, reference)| reference.strip_prefix("refs/tags/"))
            .map(|name| name.strip_suffix("^{}").unwrap_or(name))
            .collect();
        self.tags
            .iter()
            .filter(|name| !on_remote.contains(name.as_str()))
            .cloned()
            .collect()
    }
}

/// A derived local release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalReleasePlan {
    pub intent: ReleaseIntentV1,
    /// A preview only: `origin` has no push URL, so tags carry no remote and a run would refuse.
    pub tags_unbound: bool,
    /// Advisory findings from derivation (e.g. an artifact owner selected without its product).
    pub diagnostics: Vec<callisto_model::Diagnostic>,
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
    plan_release(&workspace, selections, mode, true)
}

/// The tagged releases on HEAD whose operations are not all observed satisfied: a rerun finishes them.
///
/// A tag proves only that the tag was created. Releases tagged on an older commit are settled and
/// never observed, so only a rerun at the release commit reaches the providers.
fn incomplete_tagged_releases<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    mode: LocalReleaseSource,
) -> Result<std::collections::BTreeSet<ReleasePackageId>, GraphError> {
    let head = workspace.git_access().head_sha()?;
    let tagged: std::collections::BTreeSet<ReleasePackageId> =
        crate::commands::release_decision::releases_tagged_at(workspace, &head)?
            .into_iter()
            .map(|entry| entry.package)
            .collect();
    if tagged.is_empty() {
        return Ok(tagged);
    }
    // Without an `origin` push URL a preview cannot observe anything, so it cannot call a tagged release finished.
    if mode == LocalReleaseSource::Preview && optional_git_remote(&workspace.root, workspace.runner)?.is_none() {
        return Ok(tagged);
    }
    let profile = ExecutionTrustProfileV1::GitCommit;
    let source = match mode {
        LocalReleaseSource::Preview => SourceIdentity::GitCommit { sha: head },
        LocalReleaseSource::Trusted => observe_source(workspace, profile, ReleaseCheckout::AnyHead)?,
    };
    // The roster a run would plan with every tagged release still open, so a dependency left untagged by the
    // failed run is part of it and the dependency checks see the same packages as the real derivation.
    let Some(decision) = crate::commands::release_decision::derive_release_roster(workspace, &[], &tagged)? else {
        return Ok(Default::default());
    };
    let (_, prepared) =
        derive_release_intent_with_prepared(workspace, &decision, source, profile, None, &mut Vec::new())?;
    Ok(unsatisfied_packages(
        &workspace.root,
        workspace.runner,
        &prepared,
        &tagged,
    ))
}

/// Refuses a first release whose name another project already owns.
///
/// A package this repository has never tagged has no history here, so a version of it that the registry already holds
/// under a different repository is that project's, not ours; adopting it would tag this commit as their release.
/// A package that has a tag is left alone: its published versions are its own history.
fn refuse_a_name_another_project_owns<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    decision: &callisto_model::ReleaseDecisionV1,
    source: &SourceIdentity,
    profile: ExecutionTrustProfileV1,
    mode: LocalReleaseSource,
) -> Result<(), GraphError> {
    let tags = workspace.tags()?;
    let first_releases: std::collections::BTreeSet<ReleasePackageId> = decision
        .entries
        .iter()
        .filter(|entry| {
            crate::commands::release_decision::workspace_package(workspace, &entry.package)
                .is_some_and(|package| tags.last_tag(&package.id).is_none())
        })
        .map(|entry| entry.package.clone())
        .collect();
    // Without an `origin` push URL a preview cannot tell whose repository the registry names.
    if first_releases.is_empty()
        || (mode == LocalReleaseSource::Preview && optional_git_remote(&workspace.root, workspace.runner)?.is_none())
    {
        return Ok(());
    }
    let (_, prepared) =
        derive_release_intent_with_prepared(workspace, decision, source.clone(), profile, None, &mut Vec::new())?;
    match names_taken_by_other_projects(&workspace.root, workspace.runner, &prepared, &first_releases)?
        .into_iter()
        .next()
    {
        Some(taken) => Err(GraphError::ReleaseNameTaken {
            package: taken.package.to_string(),
            version: taken.version,
            registry_repository: taken.registry_repository,
            origin: taken.origin,
        }),
        None => Ok(()),
    }
}

/// Refuses to tag a version the registry already holds while its package has moved on since the commit that set it.
///
/// A release that stopped after publishing leaves exactly this state. A rerun on a later commit would tag code that
/// was never published, so it finishes from the version commit instead. Nothing is observed unless a package moved.
fn refuse_a_published_version_left_behind<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    decision: &callisto_model::ReleaseDecisionV1,
    resumable: &std::collections::BTreeSet<ReleasePackageId>,
    source: &SourceIdentity,
    profile: ExecutionTrustProfileV1,
    mode: LocalReleaseSource,
) -> Result<(), GraphError> {
    let moved = super::version_commit::packages_moved_past_their_version(workspace, decision, resumable)?;
    // Without an `origin` push URL a preview cannot observe the registry either.
    if moved.is_empty()
        || (mode == LocalReleaseSource::Preview && optional_git_remote(&workspace.root, workspace.runner)?.is_none())
    {
        return Ok(());
    }
    let (_, prepared) =
        derive_release_intent_with_prepared(workspace, decision, source.clone(), profile, None, &mut Vec::new())?;
    let among = moved.iter().map(|moved| moved.package.clone()).collect();
    let published = published_packages(&workspace.root, workspace.runner, &prepared, &among)?;
    match moved.into_iter().find(|moved| published.contains(&moved.package)) {
        Some(moved) => {
            let (since, fix) = match moved.version_commit {
                Some(commit) => {
                    let short: String = commit.chars().take(12).collect();
                    (
                        format!("{short}, the commit that set it"),
                        format!("Check out {short} (`git checkout --detach {short}`) and run `callisto release` again to finish that release. Releasing from here would tag code that was not published."),
                    )
                }
                None => (
                    "the commit that set it, which this history does not show".to_owned(),
                    "Check out the commit that was published and run `callisto release` again, or tag that commit by hand. Releasing from here could tag code that was not published.".to_owned(),
                ),
            };
            Err(GraphError::ReleasePublishedVersionMoved {
                package: moved.package.to_string(),
                version: moved.version,
                since,
                fix,
            })
        }
        None => Ok(()),
    }
}

/// [`plan_local_release`] over an already-loaded workspace, e.g. one built from an in-memory config.
///
/// This looks at no registry for a name another project owns, so a caller that must stay offline, such as the plan
/// `init` previews, can use it; [`plan_local_release`] does look.
pub fn plan_workspace_release<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    selections: &[ReleasePackageId],
    mode: LocalReleaseSource,
) -> Result<Option<LocalReleasePlan>, GraphError> {
    plan_release(workspace, selections, mode, false)
}

/// [`plan_local_release`] over an already-loaded workspace: the same plan, including the registry checks, without
/// loading the workspace again.
pub fn plan_loaded_release<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    selections: &[ReleasePackageId],
    mode: LocalReleaseSource,
) -> Result<Option<LocalReleasePlan>, GraphError> {
    plan_release(workspace, selections, mode, true)
}

fn plan_release<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    selections: &[ReleasePackageId],
    mode: LocalReleaseSource,
    check_taken_names: bool,
) -> Result<Option<LocalReleasePlan>, GraphError> {
    let resumable = incomplete_tagged_releases(workspace, mode)?;
    let Some(decision) = crate::commands::release_decision::derive_release_roster(workspace, selections, &resumable)?
    else {
        return Ok(None);
    };
    let profile = ExecutionTrustProfileV1::GitCommit;
    let source = match mode {
        LocalReleaseSource::Preview => SourceIdentity::GitCommit {
            sha: workspace.git_access().head_sha()?,
        },
        LocalReleaseSource::Trusted => observe_source(workspace, profile, ReleaseCheckout::AnyHead)?,
    };
    if check_taken_names {
        refuse_a_name_another_project_owns(workspace, &decision, &source, profile, mode)?;
    }
    refuse_a_published_version_left_behind(workspace, &decision, &resumable, &source, profile, mode)?;
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
    let mut diagnostics = Vec::new();
    let intent = derive_release_intent(
        workspace,
        &decision,
        source.clone(),
        profile,
        artifact_policy.as_ref(),
        remote,
        &mut diagnostics,
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
    Ok(Some(LocalReleasePlan {
        intent,
        tags_unbound,
        diagnostics,
    }))
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
    fn a_tag_on_an_older_commit_means_nothing_to_release() {
        let (dir, _) = fixture();
        git(dir.path(), &["-c", "tag.gpgSign=false", "tag", "release-fixture@1.2.3"]);
        git(
            dir.path(),
            &[
                "-c",
                "commit.gpgSign=false",
                "commit",
                "--allow-empty",
                "-qm",
                "later work",
            ],
        );
        assert!(plan(dir.path(), &[], LocalReleaseSource::Preview).unwrap().is_none());
        assert!(plan(dir.path(), &[], LocalReleaseSource::Trusted).unwrap().is_none());
    }

    #[test]
    fn a_tag_on_head_without_the_rest_of_the_release_is_still_a_release() {
        let (dir, _) = fixture();
        git(dir.path(), &["-c", "tag.gpgSign=false", "tag", "release-fixture@1.2.3"]);
        for mode in [LocalReleaseSource::Preview, LocalReleaseSource::Trusted] {
            let plan = plan_offline(dir.path(), &[], mode)
                .unwrap()
                .expect("the tag alone does not finish the release");
            assert_eq!(plan.intent.decision.entries[0].package, fixture_id(), "{mode:?}");
        }
    }

    /// A preview with no `origin` cannot observe anything, so it never calls a tagged release finished;
    /// the run itself then fails for lack of a remote, as it does for any release.
    #[test]
    fn a_preview_without_origin_keeps_a_tagged_release_in_the_plan() {
        let dir = app_and_lib_repo();
        git(dir.path(), &["tag", "-a", "-m", "Release app@1.0.0", "app@1.0.0"]);
        git(dir.path(), &["remote", "remove", "origin"]);
        let plan = plan_offline(dir.path(), &[], LocalReleaseSource::Preview)
            .unwrap()
            .expect("an unobservable tagged release stays planned");
        assert_eq!(released_names(&plan), ["app", "lib"]);
        assert!(plan.tags_unbound);
    }

    /// Real git, except `ls-remote` answers "no such ref" and every other program is unavailable: a
    /// provider observation never reaches the network, and counts as not satisfied.
    struct OfflineRunner;
    impl callisto_model::CommandRunner for OfflineRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            cwd: &Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            if program != "git" {
                return Err(callisto_model::CommandError::NotFound {
                    program: program.to_owned(),
                });
            }
            if args.first() == Some(&"ls-remote") {
                return Ok(callisto_model::CommandOutput {
                    exit_code: Some(0),
                    stdout: String::new(),
                    stderr: String::new(),
                });
            }
            RealGitRunner.run(program, args, cwd)
        }
    }

    /// Crates `app` (depends on `lib`) and `lib`, both unreleased, with a GitHub `origin`, at a detached HEAD.
    fn app_and_lib_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"app\", \"lib\"]\n").unwrap();
        std::fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"cargo/app\"\npublish-to = [\"crates-io\"]\n\n[[package]]\nmatch = \"cargo/lib\"\npublish-to = [\"crates-io\"]\n",
        )
        .unwrap();
        for (name, dependencies) in [("app", "[dependencies]\nlib = { path = \"../lib\" }\n"), ("lib", "")] {
            std::fs::create_dir_all(root.join(name).join("src")).unwrap();
            std::fs::write(root.join(name).join("src/lib.rs"), "\n").unwrap();
            std::fs::write(
                root.join(name).join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n{dependencies}"),
            )
            .unwrap();
        }
        callisto_fixtures::git::init_repo(root);
        git(
            root,
            &["remote", "add", "origin", "https://github.com/example/app-and-lib.git"],
        );
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "fixture"]);
        git(root, &["checkout", "--detach", "-q", "HEAD"]);
        dir
    }

    fn plan_offline(
        root: &Path,
        selections: &[ReleasePackageId],
        mode: LocalReleaseSource,
    ) -> Result<Option<LocalReleasePlan>, GraphError> {
        plan_local_release(
            root,
            &crate::IgnoreWalkLocator::new(root),
            &OfflineRunner,
            selections,
            mode,
        )
    }

    fn released_names(plan: &LocalReleasePlan) -> Vec<String> {
        plan.intent
            .decision
            .entries
            .iter()
            .map(|entry| entry.package.name().to_owned())
            .collect()
    }

    /// The tag operations run dependents first, so a failed push can leave only the dependent's tag behind.
    /// The rerun finishes it together with the dependency that never got its tag.
    #[test]
    fn a_tagged_dependent_is_resumed_together_with_its_untagged_dependency() {
        let dir = app_and_lib_repo();
        git(dir.path(), &["tag", "-a", "-m", "Release app@1.0.0", "app@1.0.0"]);
        for mode in [LocalReleaseSource::Preview, LocalReleaseSource::Trusted] {
            let plan = plan_offline(dir.path(), &[], mode)
                .unwrap()
                .expect("unfinished release");
            assert_eq!(released_names(&plan), ["app", "lib"], "{mode:?}");
        }
    }

    #[test]
    fn trusted_run_accepts_a_branch_and_records_head() {
        let (dir, _) = fixture();
        // fixture() already leaves HEAD detached on a pre-existing `main` (from
        // init_repo); this test reattaches to it rather than creating it.
        git(dir.path(), &["checkout", "-q", "main"]);
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
        git(
            dir.path(),
            &[
                "-c",
                "commit.gpgSign=false",
                "commit",
                "--allow-empty",
                "-qm",
                "later work",
            ],
        );
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
            format!("[[{kind}-group]]\nname = \"g\"\nmembers = [\"cargo/a\", \"cargo/b\"]\n\n[[package]]\nmatch = \"cargo/*\"\npublish-to = [\"crates-io\"]\n"),
        )
        .unwrap();
        callisto_fixtures::git::init_repo(root);
        for args in [
            ["remote", "add", "origin", "https://github.com/example/grouped.git"].as_slice(),
            ["add", "."].as_slice(),
            ["commit", "-q", "-m", "fixture"].as_slice(),
        ] {
            git(root, args);
        }
        dir
    }

    /// `--package` keeps every fixed- or linked-group member, as the CI route does.
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
        callisto_fixtures::git::init_repo(napi.path());
        assert!(matches!(
            route(napi.path()),
            Some(CiReleaseRoute::PlatformPackage { .. })
        ));
    }
}
