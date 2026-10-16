use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use callisto_model::{
    CommandError, CommandOutput, CommandRunner, DepEdge, GroupKind, GroupName, ManifestDecl, ManifestFormat,
    ManifestRole, Package,
};

use crate::config::{GroupDef, GroupMember};
use crate::infer::{InferenceOutcome, InferenceWindowSpec, SeverityInference};
use callisto_fixtures::git::{init_repo, run_git};

/// Direct unit coverage for `apply_pre_major` across all three policy
/// states, previously only exercised indirectly through full-config
/// integration tests. `Off` never downgrades; `Conservative` downgrades
/// Major->Minor only; `ConservativeFeat` downgrades both Major->Minor
/// and Minor->Patch.
#[test]
fn apply_pre_major_off_never_downgrades() {
    let v = Version::semver(0, 1, 0);
    assert_eq!(
        apply_pre_major(Severity::Major, PreMajorInferencePolicy::Off, &v, true),
        (Severity::Major, false)
    );
    assert_eq!(
        apply_pre_major(Severity::Minor, PreMajorInferencePolicy::Off, &v, true),
        (Severity::Minor, false)
    );
}

#[test]
fn apply_pre_major_conservative_downgrades_major_to_minor_only() {
    let v = Version::semver(0, 1, 0);
    assert_eq!(
        apply_pre_major(Severity::Major, PreMajorInferencePolicy::Conservative, &v, true),
        (Severity::Minor, true)
    );
    assert_eq!(
        apply_pre_major(Severity::Minor, PreMajorInferencePolicy::Conservative, &v, true),
        (Severity::Minor, false),
        "Conservative must not also downgrade Minor->Patch"
    );
}

#[test]
fn apply_pre_major_conservative_feat_downgrades_both_levels() {
    let v = Version::semver(0, 1, 0);
    assert_eq!(
        apply_pre_major(Severity::Major, PreMajorInferencePolicy::ConservativeFeat, &v, true),
        (Severity::Minor, true)
    );
    assert_eq!(
        apply_pre_major(Severity::Minor, PreMajorInferencePolicy::ConservativeFeat, &v, true),
        (Severity::Patch, true)
    );
}

/// Shells out to the real `git` binary.
struct RealGitRunner;

impl CommandRunner for RealGitRunner {
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
        let output = std::process::Command::new(program)
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|e| CommandError::Io {
                program: program.to_string(),
                message: e.to_string(),
            })?;
        Ok(CommandOutput {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// A `CommandRunner` double that answers `git rev-parse --verify --quiet
/// <tag>^{commit}` with a canned SHA and counts invocations.
struct FakeRevParseRunner {
    calls: AtomicUsize,
    tag: String,
    sha: CommitSha,
}

impl CommandRunner for FakeRevParseRunner {
    fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
        assert_eq!(program, "git");
        assert_eq!(
            args,
            [
                "rev-parse",
                "--verify",
                "--quiet",
                format!("{}^{{commit}}", self.tag).as_str()
            ]
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(CommandOutput {
            exit_code: Some(0),
            stdout: format!("{}\n", self.sha.as_str()),
            stderr: String::new(),
        })
    }
}

/// Spec: `load_changesets` must include the filename in its error when a changeset file
/// fails `parse_changeset`. The bare `?` propagation previously produced a
/// `GraphError::Format(ParseError)` with no path context, making it impossible for a
/// developer to triage which file caused the failure in a workspace with many changesets.
#[test]
fn test_load_changesets_error_includes_filename() {
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();
    let cs_dir = root.join(".changeset");
    std::fs::create_dir_all(&cs_dir).unwrap();

    // Missing `---` frontmatter delimiter — parse_changeset returns
    // ParseError::MissingFrontmatterStart. The error must carry the filename so the
    // developer can find the broken file.
    std::fs::write(cs_dir.join("malformed-changeset.md"), "cargo/foo: patch\n\nSummary.\n").unwrap();

    let cfg = crate::config::load(root).unwrap();
    let result = load_changesets(root, &cfg);

    let err = result.expect_err("load_changesets must return Err for a malformed changeset file");
    let err_display = format!("{err}");
    assert!(
        err_display.contains("malformed-changeset"),
        "error message must contain the offending filename so the developer can triage; \
             got: {err_display:?}"
    );
}

/// Spec: `resolve_since` resolves the tag with one `git rev-parse
/// --verify --quiet <tag>^{commit}` rather than degrading to `None` (an
/// unbounded full-history walk).
#[test]
fn test_resolve_since_resolves_the_tag_through_git() {
    let dir = tempfile::tempdir().unwrap();
    let sha = CommitSha::parse("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef").unwrap();
    let runner = FakeRevParseRunner {
        calls: AtomicUsize::new(0),
        tag: "pkg-a@1.0.0".to_string(),
        sha: sha.clone(),
    };
    let git = GitAccess::new(dir.path(), &runner);

    let resolved = resolve_since(&git, "pkg-a@1.0.0");

    assert_eq!(
        resolved,
        Some(sha),
        "resolve_since must resolve the tag, not silently return None"
    );
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
}

/// An `IdentityIndex` reflecting `pkg`'s own canonical manifests, as
/// `ManifestWalkResolver::build` would populate it for a real workspace --
/// `resolve_target_package` resolves through this, not through `pkg.id`
/// directly, so every `aggregate()` test needs one to resolve its
/// changeset entries at all.
fn identity_for(pkg: &Package) -> IdentityIndex {
    identity_for_all(std::iter::once(pkg))
}

/// [`identity_for`] over several packages at once, for the
/// `resolve_target_package` cross-ecosystem-ambiguity tests below.
fn identity_for_all<'a>(packages: impl IntoIterator<Item = &'a Package>) -> IdentityIndex {
    let mut index = IdentityIndex::default();
    for pkg in packages {
        for manifest in pkg.canonical_manifests() {
            index
                .native
                .insert((manifest.ecosystem(), pkg.id.name().to_string()), pkg.id.clone());
        }
    }
    index
}

struct SinglePackageGraph {
    pkg: Package,
}

impl DependencyResolver for SinglePackageGraph {
    fn packages(&self) -> impl Iterator<Item = &Package> {
        std::iter::once(&self.pkg)
    }

    fn dependencies_of(&self, _id: &PackageId) -> impl Iterator<Item = &DepEdge> {
        std::iter::empty()
    }

    fn dependents_of(&self, _id: &PackageId) -> impl Iterator<Item = &DepEdge> {
        std::iter::empty()
    }
}

struct TwoPackageGraph {
    packages: Vec<Package>,
}

impl DependencyResolver for TwoPackageGraph {
    fn packages(&self) -> impl Iterator<Item = &Package> {
        self.packages.iter()
    }

    fn dependencies_of(&self, _id: &PackageId) -> impl Iterator<Item = &DepEdge> {
        std::iter::empty()
    }

    fn dependents_of(&self, _id: &PackageId) -> impl Iterator<Item = &DepEdge> {
        std::iter::empty()
    }
}

/// Records the `since` value passed into `InferenceWindowSpec` without
/// doing any real inference work.
#[derive(Default)]
struct RecordingInference {
    captured_since: Mutex<Option<CommitSha>>,
}

impl SeverityInference for RecordingInference {
    fn infer(
        &self,
        _pkg: &Package,
        _git: &GitAccess<'_>,
        window: InferenceWindowSpec<'_>,
    ) -> Result<Option<InferenceOutcome>, GraphError> {
        *self.captured_since.lock().unwrap() = window.since.clone();
        Ok(None)
    }
}

/// Spec: `aggregate()` must scope commit inference to `last_tag..HEAD`
/// instead of walking full history on every run. Reproduces the bug by
/// building a real one-package repo with a real release tag, then
/// asserting the `since` field handed to `SeverityInference::infer`
/// carries the commit SHA the tag points at (not `None`, which forces a
/// full-history walk in `crate::conventional::window::fetch_commits`).
#[test]
fn test_aggregate_scopes_inference_window_to_last_tag() {
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    init_repo(root);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial commit"]);

    let pkg_id = PackageId::parse("pkg-a").unwrap();
    let tag_name = format!("{}@1.0.0", pkg_id.display_name());
    // Explicit message + disabled gpg signing so this is robust
    // regardless of the developer machine's global git config (e.g.
    // `tag.forceSignAnnotated` / `tag.gpgSign`).
    run_git(root, &["-c", "tag.gpgSign=false", "tag", "-m", "release", &tag_name]);

    // A commit landing after the tag; a correctly-scoped inference
    // window must never need to look past `tag_name` to find it, but a
    // `since: None` (full history) window would happily walk right over
    // it and beyond, all the way back to the repo root.
    std::fs::write(root.join("CHANGES.md"), "more\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "feat: add changes file"]);

    let expected_sha_output = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", &format!("{tag_name}^{{commit}}")])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(expected_sha_output.status.success());
    let expected_sha = CommitSha::parse(String::from_utf8_lossy(&expected_sha_output.stdout).trim()).unwrap();

    let runner = RealGitRunner;
    let manifest = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let graph = SinglePackageGraph {
        pkg: Package {
            id: pkg_id.clone(),
            manifests: vec![manifest],
            changelog: None,
            release_trigger: callisto_model::ReleaseTrigger::Auto,
            publish_to: Vec::new(),
            tag_template: None,
        },
    };
    let git = GitAccess::new(root, &runner);
    let cfg = crate::config::load(root).unwrap();
    let tags = TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();

    // Sanity: the tag we just created was actually picked up.
    assert_eq!(
        tags.last_tag(&pkg_id).map(|t| t.version.render().to_string()),
        Some("1.0.0".to_string())
    );

    let inference = RecordingInference::default();
    let base_versions = BTreeMap::new();

    aggregate(
        &graph,
        &identity_for(&graph.pkg),
        &cfg,
        &git,
        &tags,
        &base_versions,
        None,
        &inference,
    )
    .unwrap();

    let captured = inference.captured_since.lock().unwrap().clone();
    assert_eq!(
        captured,
        Some(expected_sha),
        "aggregate() must scope inference to last_tag..HEAD instead of hardcoding `since: None` \
             (full history)"
    );
}

/// Records the `pathspecs` value passed into `InferenceWindowSpec`
/// without doing any real inference work.
#[derive(Default)]
struct RecordingPathspecsInference {
    captured_pathspecs: Mutex<Vec<PathBuf>>,
}

impl SeverityInference for RecordingPathspecsInference {
    fn infer(
        &self,
        _pkg: &Package,
        _git: &GitAccess<'_>,
        window: InferenceWindowSpec<'_>,
    ) -> Result<Option<InferenceOutcome>, GraphError> {
        *self.captured_pathspecs.lock().unwrap() = window.pathspecs.to_vec();
        Ok(None)
    }
}

/// Spec: `aggregate()` must scope commit inference to the package's
/// source *directory*, not the manifest *file* path -- otherwise
/// inference only ever matches commits touching the manifest itself,
/// never real source changes, making it a near-total no-op. Reproduces
/// the bug directly: a package whose canonical manifest lives at
/// `crates/pkg-a/Cargo.toml` must produce a pathspec of `crates/pkg-a`
/// (mirroring `changed::package_paths`'s directory-scoping convention),
/// not `crates/pkg-a/Cargo.toml`.
#[test]
fn test_aggregate_scopes_inference_pathspecs_to_package_directory() {
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();
    init_repo(root);

    let pkg_id = PackageId::parse("pkg-a").unwrap();
    let manifest = ManifestDecl::new(
        "crates/pkg-a/Cargo.toml",
        ManifestRole::Canonical,
        ManifestFormat::CargoToml,
    )
    .unwrap();
    let graph = SinglePackageGraph {
        pkg: Package {
            id: pkg_id.clone(),
            manifests: vec![manifest],
            changelog: None,
            release_trigger: callisto_model::ReleaseTrigger::Auto,
            publish_to: Vec::new(),
            tag_template: None,
        },
    };

    let runner = RealGitRunner;
    let git = GitAccess::new(root, &runner);
    let cfg = crate::config::load(root).unwrap();
    let tags = TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();

    let inference = RecordingPathspecsInference::default();
    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_id, Version::semver(0, 1, 0));

    aggregate(
        &graph,
        &identity_for(&graph.pkg),
        &cfg,
        &git,
        &tags,
        &base_versions,
        None,
        &inference,
    )
    .unwrap();

    let captured = inference.captured_pathspecs.lock().unwrap().clone();
    assert_eq!(
        captured,
        vec![PathBuf::from("crates/pkg-a")],
        "aggregate() must scope inference pathspecs to the package's source directory, not the \
             manifest file path"
    );
}

struct FixedCommitsInference {
    commits: Vec<(CommitSha, String)>,
}

impl SeverityInference for FixedCommitsInference {
    fn infer(
        &self,
        _pkg: &Package,
        _git: &GitAccess<'_>,
        _window: InferenceWindowSpec<'_>,
    ) -> Result<Option<InferenceOutcome>, GraphError> {
        Ok(Some(InferenceOutcome {
            severity: Severity::Minor,
            commit_count: self.commits.len(),
            remapped: false,
            commits: self.commits.clone(),
        }))
    }
}

/// aggregate() must retain InferenceOutcome.commits
/// on Aggregation.inference_commits keyed by package, not discard it
/// after constructing BumpReason::Inference (which only carries a count).
#[test]
fn test_aggregate_retains_inference_commits_on_aggregation() {
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();
    init_repo(root);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial commit"]);

    let pkg_id = PackageId::parse("pkg-a").unwrap();
    let manifest = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let graph = SinglePackageGraph {
        pkg: Package {
            id: pkg_id.clone(),
            manifests: vec![manifest],
            changelog: None,
            release_trigger: callisto_model::ReleaseTrigger::Auto,
            publish_to: Vec::new(),
            tag_template: None,
        },
    };
    let runner = RealGitRunner;
    let git = GitAccess::new(root, &runner);
    let cfg = crate::config::load(root).unwrap();
    let tags = TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();

    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_id.clone(), callisto_model::Version::semver(1, 0, 0));

    let sha_recent = CommitSha::parse(&"a".repeat(40)).unwrap();
    let inference = FixedCommitsInference {
        commits: vec![(sha_recent.clone(), "feat: recent".to_string())],
    };

    let agg = aggregate(
        &graph,
        &identity_for(&graph.pkg),
        &cfg,
        &git,
        &tags,
        &base_versions,
        None,
        &inference,
    )
    .unwrap();

    assert_eq!(
        agg.inference_commits.get(&pkg_id),
        Some(&vec![(sha_recent, "feat: recent".to_string())]),
        "Aggregation.inference_commits must retain InferenceOutcome.commits for the package"
    );
}

/// Records how many times `infer()` was called, without doing any real
/// inference work.
#[derive(Default)]
struct CountingInference {
    calls: AtomicUsize,
}

impl SeverityInference for CountingInference {
    fn infer(
        &self,
        _pkg: &Package,
        _git: &GitAccess<'_>,
        _window: InferenceWindowSpec<'_>,
    ) -> Result<Option<InferenceOutcome>, GraphError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
}

/// A package resolved to `ReleaseTrigger::Changeset` (the default) must not
/// have commit-based severity inference invoked at all, even when the `inference`
/// feature is compiled in -- its severity comes only from pending changesets.
#[test]
fn test_aggregate_skips_inference_for_changeset_trigger() {
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();
    init_repo(root);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial commit"]);

    let pkg_id = PackageId::parse("pkg-a").unwrap();
    let manifest = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let graph = SinglePackageGraph {
        pkg: Package {
            id: pkg_id.clone(),
            manifests: vec![manifest],
            changelog: None,
            release_trigger: callisto_model::ReleaseTrigger::Changeset,
            publish_to: Vec::new(),
            tag_template: None,
        },
    };
    let runner = RealGitRunner;
    let git = GitAccess::new(root, &runner);
    let cfg = crate::config::load(root).unwrap();
    let tags = TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();

    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_id, callisto_model::Version::semver(1, 0, 0));

    let inference = CountingInference::default();
    aggregate(
        &graph,
        &identity_for(&graph.pkg),
        &cfg,
        &git,
        &tags,
        &base_versions,
        None,
        &inference,
    )
    .unwrap();

    assert_eq!(
        inference.calls.load(Ordering::SeqCst),
        0,
        "aggregate() must not invoke SeverityInference::infer for a package whose \
             resolved release_trigger is ReleaseTrigger::Changeset"
    );
}

/// A package resolved to `ReleaseTrigger::Auto` must still run commit-based
/// severity inference, and its outcome can still raise the package's severity above
/// what pending changesets alone would produce -- unchanged from current behavior.
#[test]
fn test_aggregate_runs_inference_for_auto_trigger_and_raises_severity() {
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();
    init_repo(root);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial commit"]);

    let pkg_id = PackageId::parse("pkg-a").unwrap();
    let manifest = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let graph = SinglePackageGraph {
        pkg: Package {
            id: pkg_id.clone(),
            manifests: vec![manifest],
            changelog: None,
            release_trigger: callisto_model::ReleaseTrigger::Auto,
            publish_to: Vec::new(),
            tag_template: None,
        },
    };
    let runner = RealGitRunner;
    let git = GitAccess::new(root, &runner);
    let cfg = crate::config::load(root).unwrap();
    let tags = TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();

    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_id.clone(), callisto_model::Version::semver(1, 0, 0));

    // No changesets on disk: with no other signal, severity stays `None`
    // unless inference itself raises it.
    let inference = FixedCommitsInference {
        commits: vec![(CommitSha::parse(&"b".repeat(40)).unwrap(), "feat: auto".to_string())],
    };

    let agg = aggregate(
        &graph,
        &identity_for(&graph.pkg),
        &cfg,
        &git,
        &tags,
        &base_versions,
        None,
        &inference,
    )
    .unwrap();

    assert_eq!(
        agg.severities.get(&pkg_id),
        Some(&Severity::Minor),
        "aggregate() must invoke inference for ReleaseTrigger::Auto and let its outcome \
             raise the package's severity"
    );
}

/// Spec: `aggregate()` scopes inference to the commit the last tag
/// points at.
#[test]
fn test_aggregate_resolves_since_to_the_tag_commit() {
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    init_repo(root);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial commit"]);

    let pkg_id = PackageId::parse("pkg-a").unwrap();
    let tag_name = format!("{}@1.0.0", pkg_id.display_name());
    run_git(root, &["-c", "tag.gpgSign=false", "tag", "-m", "release", &tag_name]);

    std::fs::write(root.join("CHANGES.md"), "more\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "feat: add changes file"]);

    let expected_sha_output = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", &format!("{tag_name}^{{commit}}")])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(expected_sha_output.status.success());
    let expected_sha = CommitSha::parse(String::from_utf8_lossy(&expected_sha_output.stdout).trim()).unwrap();

    let runner = RealGitRunner;
    let manifest = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let graph = SinglePackageGraph {
        pkg: Package {
            id: pkg_id.clone(),
            manifests: vec![manifest],
            changelog: None,
            release_trigger: callisto_model::ReleaseTrigger::Auto,
            publish_to: Vec::new(),
            tag_template: None,
        },
    };
    let git = GitAccess::new(root, &runner);
    let cfg = crate::config::load(root).unwrap();
    let tags = TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();

    assert_eq!(
        tags.last_tag(&pkg_id).map(|t| t.version.render().to_string()),
        Some("1.0.0".to_string())
    );

    let inference = RecordingInference::default();
    let base_versions = BTreeMap::new();

    aggregate(
        &graph,
        &identity_for(&graph.pkg),
        &cfg,
        &git,
        &tags,
        &base_versions,
        None,
        &inference,
    )
    .unwrap();

    let captured = inference.captured_since.lock().unwrap().clone();
    assert_eq!(
        captured,
        Some(expected_sha),
        "aggregate() must resolve `since` to the last tag's commit"
    );
}

// Auto so aggregate() invokes inference for callers of this fixture that
// exercise SeverityInference; trigger itself is irrelevant to the rest.
//
// The manifest's format must match `id`'s own ecosystem: `identity_for_all`
// (and, in production, `ManifestWalkResolver::build`) derives native-name
// ecosystems from the manifest, not from the id, so a `npm/foo` id with a
// `Cargo.toml` manifest would register as Cargo and defeat the
// cross-ecosystem-ambiguity tests this fixture backs.
fn make_pkg(id: PackageId) -> Package {
    let (filename, format) = match id.ecosystem() {
        Some(Ecosystem::Npm) => ("package.json", ManifestFormat::PackageJson),
        Some(Ecosystem::Pypi) => ("pyproject.toml", ManifestFormat::PyprojectToml),
        _ => ("Cargo.toml", ManifestFormat::CargoToml),
    };
    let manifest = ManifestDecl::new(filename, ManifestRole::Canonical, format).unwrap();
    Package {
        id,
        manifests: vec![manifest],
        changelog: None,
        release_trigger: callisto_model::ReleaseTrigger::Auto,
        publish_to: Vec::new(),
        tag_template: None,
    }
}

/// Spec: a changeset entry naming a package by its bare name (no
/// ecosystem prefix) must NOT silently resolve against an arbitrary
/// candidate when the graph contains packages in two or more ecosystems
/// sharing that name. Resolving `foo` against both `cargo/foo` and
/// `npm/foo` is genuinely ambiguous and must be a caller-visible error,
/// not a first-match-wins pick based on iteration order.
#[test]
fn test_resolve_target_package_ambiguous_bare_name_errors() {
    let pkg_cargo = make_pkg(PackageId::parse("cargo/foo").unwrap());
    let pkg_npm = make_pkg(PackageId::parse("npm/foo").unwrap());
    let packages = [pkg_cargo, pkg_npm];
    let identity = identity_for_all(&packages);
    let bare = PackageId::parse("foo").unwrap();

    let result = resolve_target_package(packages.iter(), &identity, &bare);

    match result {
        Err(GraphError::AmbiguousName { name, candidates }) => {
            assert_eq!(name, "foo");
            assert_eq!(candidates.len(), 2);
            assert!(candidates.contains(&PackageId::parse("cargo/foo").unwrap()));
            assert!(candidates.contains(&PackageId::parse("npm/foo").unwrap()));
        }
        other => panic!("expected GraphError::AmbiguousName, got {other:?}"),
    }
}

/// Spec: a bare-name lookup must still resolve fine when the name is
/// unambiguous (only one package with that name across all ecosystems
/// in the graph).
#[test]
fn test_resolve_target_package_unambiguous_bare_name_resolves() {
    let pkg_cargo = make_pkg(PackageId::parse("cargo/foo").unwrap());
    let pkg_other = make_pkg(PackageId::parse("cargo/bar").unwrap());
    let packages = [pkg_cargo, pkg_other];
    let identity = identity_for_all(&packages);
    let bare = PackageId::parse("foo").unwrap();

    let result = resolve_target_package(packages.iter(), &identity, &bare).unwrap();

    assert_eq!(
        result.map(|p| p.id.clone()),
        Some(PackageId::parse("cargo/foo").unwrap())
    );
}

/// Spec: a bare-name lookup for a name that doesn't exist anywhere in
/// the graph resolves to `None` (not an error) -- unknown-package
/// reporting is the caller's responsibility (see validate.rs).
#[test]
fn test_resolve_target_package_unknown_name_returns_none() {
    let pkg_cargo = make_pkg(PackageId::parse("cargo/foo").unwrap());
    let packages = [pkg_cargo];
    let identity = identity_for_all(&packages);
    let bare = PackageId::parse("does-not-exist").unwrap();

    let result = resolve_target_package(packages.iter(), &identity, &bare).unwrap();

    assert!(result.is_none());
}

/// Spec: the ambiguity check must not assume exactly two colliding
/// candidates. A workspace with the same bare name registered in three
/// or more ecosystems (cargo/foo, npm/foo, pypi/foo) must still report
/// every candidate in `AmbiguousName`, not just the first two (an
/// off-by-one truncation or a hardcoded pairwise assumption would not
/// be caught by the two-ecosystem test above).
#[test]
fn test_resolve_target_package_ambiguous_bare_name_three_ecosystems_errors() {
    let pkg_cargo = make_pkg(PackageId::parse("cargo/foo").unwrap());
    let pkg_npm = make_pkg(PackageId::parse("npm/foo").unwrap());
    let pkg_pypi = make_pkg(PackageId::parse("pypi/foo").unwrap());
    let packages = [pkg_cargo, pkg_npm, pkg_pypi];
    let identity = identity_for_all(&packages);
    let bare = PackageId::parse("foo").unwrap();

    let result = resolve_target_package(packages.iter(), &identity, &bare);

    match result {
        Err(GraphError::AmbiguousName { name, candidates }) => {
            assert_eq!(name, "foo");
            assert_eq!(candidates.len(), 3);
            assert!(candidates.contains(&PackageId::parse("cargo/foo").unwrap()));
            assert!(candidates.contains(&PackageId::parse("npm/foo").unwrap()));
            assert!(candidates.contains(&PackageId::parse("pypi/foo").unwrap()));
        }
        other => panic!("expected GraphError::AmbiguousName with 3 candidates, got {other:?}"),
    }
}

/// Spec: bare-name matching against `PackageId::name()` is a plain
/// string comparison, which is case-sensitive. A package registered as
/// `cargo/Foo` must NOT be resolved by a bare lookup for `foo` -- they
/// are treated as distinct names, so the lookup resolves to `None`
/// (unknown-package) rather than matching or erroring as ambiguous.
/// This test pins down that actual behavior explicitly so a future
/// change to case handling is a deliberate, visible decision.
#[test]
fn test_resolve_target_package_bare_name_matching_is_case_sensitive() {
    let pkg_cargo = make_pkg(PackageId::parse("cargo/Foo").unwrap());
    let packages = [pkg_cargo];
    let identity = identity_for_all(&packages);
    let bare = PackageId::parse("foo").unwrap();

    let result = resolve_target_package(packages.iter(), &identity, &bare).unwrap();

    assert!(
        result.is_none(),
        "case-sensitive name comparison must not match 'foo' against 'Foo'"
    );
}

/// Spec: a changeset where EVERY entry references a package not in the
/// workspace must NOT be added to `consumed` (which would silently delete
/// it on disk) and must emit a `DiagnosticCode::UnknownPackage` warning
/// for each orphaned entry.  On the current (unfixed) code, the changeset
/// IS added to `consumed` before the entry loop, so it ends up deleted
/// despite no version bump ever being recorded.
#[test]
fn test_orphaned_changeset_not_consumed_emits_unknown_package_diagnostic() {
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    // Minimal git repo so TagIndex::build can enumerate tags.
    init_repo(root);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial commit"]);

    // Changeset referencing only pkg-foo which is NOT in the workspace.
    let cs_dir = root.join(".changeset");
    std::fs::create_dir_all(&cs_dir).unwrap();
    std::fs::write(
        cs_dir.join("orphan-cs.md"),
        "---\n\"pkg-foo\": minor\n---\n\nOrphaned changeset.\n",
    )
    .unwrap();

    // Workspace has only pkg-bar.
    let pkg_bar_id = PackageId::parse("pkg-bar").unwrap();
    let graph = SinglePackageGraph {
        pkg: make_pkg(pkg_bar_id.clone()),
    };
    let cfg = crate::config::load(root).unwrap();
    let runner = RealGitRunner;
    let git = GitAccess::new(root, &runner);
    let tags = crate::tags::TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();

    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_bar_id.clone(), Version::semver(1, 0, 0));

    let inference = RecordingInference::default();
    let agg = aggregate(
        &graph,
        &identity_for(&graph.pkg),
        &cfg,
        &git,
        &tags,
        &base_versions,
        None,
        &inference,
    )
    .unwrap();

    assert!(
        agg.consumed.is_empty(),
        "a fully-orphaned changeset (all entries reference non-existent packages) must NOT \
             be added to consumed (which would cause it to be deleted on disk): got {:?}",
        agg.consumed
    );

    let unknown_pkg_diags: Vec<_> = agg
        .diagnostics
        .iter()
        .filter(|d| d.code == callisto_model::DiagnosticCode::UnknownPackage)
        .collect();
    assert!(
        !unknown_pkg_diags.is_empty(),
        "must emit at least one UnknownPackage diagnostic for orphaned changeset entries; \
             got diagnostics: {:?}",
        agg.diagnostics
    );
}

/// Spec: a changeset with a MIX of a known and an unknown package entry
/// must NOT be consumed. Consuming it (as the previous code did once
/// `matched_any` went true) deletes the changeset file while the unknown
/// entry's bump is never recorded anywhere, silently losing it -- despite
/// the accompanying diagnostic claiming the changeset "will not be
/// consumed until this entry is resolved".
#[test]
fn test_changeset_with_one_unknown_entry_not_consumed() {
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    init_repo(root);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial commit"]);

    // One entry names a real workspace package, the other names a package
    // that has since been removed from the workspace.
    let cs_dir = root.join(".changeset");
    std::fs::create_dir_all(&cs_dir).unwrap();
    std::fs::write(
        cs_dir.join("mixed-cs.md"),
        "---\n\"pkg-bar\": patch\n\"pkg-removed\": minor\n---\n\nMixed changeset.\n",
    )
    .unwrap();

    let pkg_bar_id = PackageId::parse("pkg-bar").unwrap();
    let pkg_bar = make_pkg(pkg_bar_id.clone());
    let graph = TwoPackageGraph {
        packages: vec![pkg_bar.clone()],
    };
    let cfg = crate::config::load(root).unwrap();
    let runner = RealGitRunner;
    let git = GitAccess::new(root, &runner);
    let tags = crate::tags::TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();

    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_bar_id.clone(), Version::semver(1, 0, 0));

    let inference = RecordingInference::default();
    let agg = aggregate(
        &graph,
        &identity_for(&pkg_bar),
        &cfg,
        &git,
        &tags,
        &base_versions,
        None,
        &inference,
    )
    .unwrap();

    assert!(
        agg.consumed.is_empty(),
        "a changeset with any unresolved entry must NOT be added to consumed (which would \
             delete it on disk and permanently lose the unresolved entry's bump): got {:?}",
        agg.consumed
    );

    // The known entry's severity must still be tracked for this run, even
    // though the changeset itself stays on disk pending the unknown entry.
    assert_eq!(agg.severities.get(&pkg_bar_id).copied(), Some(Severity::Patch));

    let unknown_pkg_diags: Vec<_> = agg
        .diagnostics
        .iter()
        .filter(|d| d.code == callisto_model::DiagnosticCode::UnknownPackage)
        .collect();
    assert!(
        !unknown_pkg_diags.is_empty(),
        "must emit an UnknownPackage diagnostic for the unresolved entry; got diagnostics: {:?}",
        agg.diagnostics
    );
}

/// Spec: when a fixed group in callisto.toml references a package that
/// no longer exists in the workspace, `union_fixed` must NOT insert
/// the stale member into `agg.severities`. Doing so causes
/// `bump_target` in `solve_cascade` to call
/// `input.base.get(stale_id)` -> `None` ->
/// `Err(GraphError::Manifest(MissingField))`, crashing `callisto
/// version` with a misleading error. The stale member must be skipped
/// and an `UnknownPackage` warning emitted.
///
/// Setup: `pkg_bar` has `Severity::Minor` (from a changeset), `pkg_baz`
/// has none yet. Fixed group has all three: `pkg_foo` (stale),
/// `pkg_bar`, `pkg_baz`. `union_fixed` should propagate `Minor` to
/// `pkg_baz`, skip `pkg_foo` with a diagnostic, and
/// return `true` because `pkg_baz` changed.
#[test]
fn test_union_fixed_stale_member_emits_diagnostic_and_is_skipped() {
    let pkg_foo = PackageId::parse("pkg-foo").unwrap(); // stale: removed from workspace
    let pkg_bar = PackageId::parse("pkg-bar").unwrap(); // real workspace package (has severity)
    let pkg_baz = PackageId::parse("pkg-baz").unwrap(); // real workspace package (no severity yet)

    let mut agg = Aggregation::default();
    // pkg-bar has a changeset-driven Minor bump; pkg-baz has nothing yet.
    agg.severities.insert(pkg_bar.clone(), Severity::Minor);
    agg.named_by.insert(pkg_bar.clone(), NamedBy::Changeset);

    let mut groups = GroupTable::default();
    let group_def = GroupDef {
        name: GroupName("fixed-grp".to_string()),
        kind: GroupKind::Fixed,
        members: vec![
            GroupMember::Package(pkg_foo.clone()),
            GroupMember::Package(pkg_bar.clone()),
            GroupMember::Package(pkg_baz.clone()),
        ],
    };
    groups.fixed.insert(group_def.name.clone(), group_def);

    // Only pkg-bar and pkg-baz are in the workspace; pkg-foo is stale.
    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_bar.clone(), Version::semver(1, 0, 0));
    base_versions.insert(pkg_baz.clone(), Version::semver(1, 0, 0));

    let changed = union_fixed(&mut agg, &groups, &base_versions);

    // pkg_baz had no severity but should now have Minor propagated from pkg_bar.
    assert!(
        changed,
        "union_fixed must return true because pkg-baz received a propagated severity"
    );
    assert_eq!(
        agg.severities.get(&pkg_baz),
        Some(&Severity::Minor),
        "real member pkg-baz must receive the propagated Minor severity"
    );
    // The stale member must never enter severities.
    assert!(
        !agg.severities.contains_key(&pkg_foo),
        "stale group member pkg-foo must NOT be inserted into severities (would crash cascade \
             with a misleading MissingField error)"
    );

    let unknown_diags: Vec<_> = agg
        .diagnostics
        .iter()
        .filter(|d| d.code == callisto_model::DiagnosticCode::UnknownPackage)
        .collect();
    assert!(
        !unknown_diags.is_empty(),
        "must emit an UnknownPackage diagnostic for stale fixed group member; \
             got diagnostics: {:?}",
        agg.diagnostics
    );
}

fn linked_group(name: &str, members: &[PackageId]) -> GroupTable {
    let mut groups = GroupTable::default();
    let group_def = GroupDef {
        name: GroupName(name.to_string()),
        kind: GroupKind::Linked,
        members: members.iter().cloned().map(GroupMember::Package).collect(),
    };
    groups.linked.insert(group_def.name.clone(), group_def);
    groups
}

#[test]
fn test_union_linked_propagates_severity_from_named_member() {
    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();

    let mut agg = Aggregation::default();
    agg.severities.insert(pkg_b.clone(), Severity::Minor);
    agg.named_by.insert(pkg_b.clone(), NamedBy::Changeset);

    let groups = linked_group("linked-pair", &[pkg_a.clone(), pkg_b.clone()]);

    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_a.clone(), Version::semver(1, 0, 0));
    base_versions.insert(pkg_b.clone(), Version::semver(1, 0, 0));

    let changed = union_linked(&mut agg, &groups, &base_versions);

    assert!(changed);
    assert_eq!(agg.severities.get(&pkg_a), Some(&Severity::Minor));
    assert_eq!(agg.severities.get(&pkg_b), Some(&Severity::Minor));
    assert_eq!(
        agg.reasons.get(&pkg_a),
        Some(&BumpReason::LinkedGroupUnion {
            group: GroupName("linked-pair".to_string()),
        })
    );
}

#[test]
fn test_union_linked_does_not_downgrade_higher_existing_severity() {
    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();

    let mut agg = Aggregation::default();
    agg.severities.insert(pkg_a.clone(), Severity::Major);
    agg.severities.insert(pkg_b.clone(), Severity::Minor);
    agg.named_by.insert(pkg_a.clone(), NamedBy::Inference);
    agg.named_by.insert(pkg_b.clone(), NamedBy::Changeset);

    let groups = linked_group("linked-pair", &[pkg_a.clone(), pkg_b.clone()]);

    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_a.clone(), Version::semver(1, 0, 0));
    base_versions.insert(pkg_b.clone(), Version::semver(1, 0, 0));

    let changed = union_linked(&mut agg, &groups, &base_versions);

    assert!(changed);
    assert_eq!(agg.severities.get(&pkg_a), Some(&Severity::Major));
    assert_eq!(agg.severities.get(&pkg_b), Some(&Severity::Major));
}

#[test]
fn test_union_linked_noop_when_no_member_named() {
    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();

    let mut agg = Aggregation::default();
    let groups = linked_group("linked-pair", &[pkg_a.clone(), pkg_b.clone()]);

    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_a.clone(), Version::semver(1, 0, 0));
    base_versions.insert(pkg_b.clone(), Version::semver(1, 0, 0));

    let changed = union_linked(&mut agg, &groups, &base_versions);

    assert!(!changed);
    assert!(agg.severities.is_empty());
}

/// Spec: when `SeverityInference::infer` returns `Err`, `aggregate()` must emit a
/// diagnostic (warning level) describing the failure rather than silently discarding
/// the error and leaving the package with no inferred severity bump.
#[test]
fn test_aggregate_inference_error_emits_diagnostic() {
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    init_repo(root);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial commit"]);

    let pkg_id = PackageId::parse("pkg-a").unwrap();
    let graph = SinglePackageGraph {
        pkg: make_pkg(pkg_id.clone()),
    };
    let cfg = crate::config::load(root).unwrap();
    let runner = RealGitRunner;
    let git = GitAccess::new(root, &runner);
    let tags = crate::tags::TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();
    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_id.clone(), Version::semver(1, 0, 0));

    struct AlwaysErrorInference;
    impl SeverityInference for AlwaysErrorInference {
        fn infer(
            &self,
            _pkg: &Package,
            _git: &GitAccess<'_>,
            _window: InferenceWindowSpec<'_>,
        ) -> Result<Option<InferenceOutcome>, GraphError> {
            Err(GraphError::Vcs(callisto_model::vcs::VcsError::Git(
                "simulated inference failure".into(),
            )))
        }
    }

    let agg = aggregate(
        &graph,
        &identity_for(&graph.pkg),
        &cfg,
        &git,
        &tags,
        &base_versions,
        None,
        &AlwaysErrorInference,
    )
    .unwrap();

    assert!(
        !agg.diagnostics.is_empty(),
        "aggregate() must emit a diagnostic when SeverityInference::infer returns Err; got none"
    );
}

/// Spec: a bare-name `[[package]]` rule in callisto.toml must match a workspace
/// package with a prefixed ID (e.g. `cargo/pkg-a`) via `PackageId::matches()`.
/// The previous `id == &pkg.id` structural equality check was silently inert for
/// prefixed package IDs when the config rule used a bare name.
#[test]
fn test_aggregate_bare_name_config_policy_matches_prefixed_package() {
    use crate::config::resolve::PreMajorInferencePolicy;
    use std::sync::atomic::AtomicBool;

    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    init_repo(root);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial commit"]);

    // Config uses a BARE name, but the workspace package has a PREFIXED ID.
    // PackageId::matches() must bridge the gap; == does not.
    std::fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"pkg-a\"\npre-major-inference = \"conservative\"\n",
    )
    .unwrap();

    let pkg_id = PackageId::parse("cargo/pkg-a").unwrap();
    let graph = SinglePackageGraph {
        pkg: make_pkg(pkg_id.clone()),
    };
    let cfg = crate::config::load(root).unwrap();
    let runner = RealGitRunner;
    let git = GitAccess::new(root, &runner);
    let tags = crate::tags::TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();
    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_id.clone(), Version::semver(0, 1, 0));

    struct PolicyCapturingInference2 {
        saw_non_off: AtomicBool,
    }
    impl SeverityInference for PolicyCapturingInference2 {
        fn infer(
            &self,
            _pkg: &Package,
            _git: &GitAccess<'_>,
            window: InferenceWindowSpec<'_>,
        ) -> Result<Option<InferenceOutcome>, GraphError> {
            if window.policy != PreMajorInferencePolicy::Off {
                self.saw_non_off.store(true, Ordering::SeqCst);
            }
            Ok(None)
        }
    }

    let capturing = PolicyCapturingInference2 {
        saw_non_off: AtomicBool::new(false),
    };
    aggregate(
        &graph,
        &identity_for(&graph.pkg),
        &cfg,
        &git,
        &tags,
        &base_versions,
        None,
        &capturing,
    )
    .unwrap();

    assert!(
        capturing.saw_non_off.load(Ordering::SeqCst),
        "a bare-name [[package]] rule must match a prefixed package ID via \
             PackageId::matches(); the old == comparison was silently inert for \
             cargo/pkg-a when callisto.toml uses match = \"pkg-a\""
    );
}

/// Spec: during a pre-release cycle (PreMode::Pre), aggregate() must NOT populate
/// agg.consumed. Changesets must remain on disk so they can be re-applied when the
/// cycle exits. The previous code unconditionally pushed to consumed regardless of
/// the PreState passed in.
#[test]
fn test_aggregate_does_not_consume_changesets_during_pre_mode() {
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    init_repo(root);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial commit"]);

    let cs_dir = root.join(".changeset");
    std::fs::create_dir_all(&cs_dir).unwrap();
    std::fs::write(
        cs_dir.join("some-feature.md"),
        "---\n\"pkg-a\": minor\n---\n\nA feature in pre mode.\n",
    )
    .unwrap();

    let pkg_id = PackageId::parse("pkg-a").unwrap();
    let graph = SinglePackageGraph {
        pkg: make_pkg(pkg_id.clone()),
    };
    let cfg = crate::config::load(root).unwrap();
    let runner = RealGitRunner;
    let git = GitAccess::new(root, &runner);
    let tags = crate::tags::TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();
    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_id.clone(), Version::semver(1, 0, 0));

    let pre_state =
        callisto_model::format::PreState::entering("next", [("pkg-a".to_string(), Version::semver(1, 0, 0))]);

    let inference = RecordingInference::default();
    let agg = aggregate(
        &graph,
        &identity_for(&graph.pkg),
        &cfg,
        &git,
        &tags,
        &base_versions,
        Some(&pre_state),
        &inference,
    )
    .unwrap();

    assert!(
        agg.consumed.is_empty(),
        "changesets must NOT be consumed during a pre-release cycle (PreMode::Pre); \
             agg.consumed must be empty but got: {:?}",
        agg.consumed
    );
}

/// The pre-mode changelog baseline must key `initialVersions` by `PackageId::name()`, not `display_name()`.
#[test]
fn test_aggregate_pre_mode_changelog_baseline_reads_the_qualified_key() {
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    init_repo(root);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial commit"]);

    let cs_dir = root.join(".changeset");
    std::fs::create_dir_all(&cs_dir).unwrap();
    std::fs::write(
        cs_dir.join("some-feature.md"),
        "---\n\"npm/pkg-a\": minor\n---\n\nA feature in pre mode.\n",
    )
    .unwrap();

    let pkg_id = PackageId::parse("npm/pkg-a").unwrap();
    let graph = SinglePackageGraph {
        pkg: make_pkg(pkg_id.clone()),
    };
    let cfg = crate::config::load(root).unwrap();
    let runner = RealGitRunner;
    let git = GitAccess::new(root, &runner);
    let tags = crate::tags::TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();
    let mut base_versions = BTreeMap::new();
    // Far from the pinned pre-cycle baseline, so a key-lookup miss falling back to this is caught below.
    base_versions.insert(pkg_id.clone(), Version::semver(9, 9, 9));

    let pre_state =
        callisto_model::format::PreState::entering("next", [("npm/pkg-a".to_string(), Version::semver(1, 2, 3))]);

    let inference = RecordingInference::default();
    let agg = aggregate(
        &graph,
        &identity_for(&graph.pkg),
        &cfg,
        &git,
        &tags,
        &base_versions,
        Some(&pre_state),
        &inference,
    )
    .unwrap();

    let cl_input = agg
        .changelog_inputs
        .get(&pkg_id)
        .expect("changeset entry must produce a changelog input");

    assert_eq!(
        cl_input.from.render(),
        "1.2.3",
        "pre-mode changelog baseline must resolve pre.json's pinned initialVersions \
             entry via the bare-name key, not fall through to base_versions (9.9.9) because \
             a display_name() lookup (\"npm/pkg-a\") missed the bare-name (\"pkg-a\") key"
    );
}

/// Spec: `aggregate()` must pass the per-package `pre_major_inference` policy from
/// `config.packages` into `InferenceWindowSpec`, not always hardcode `OFF`.
#[test]
fn test_aggregate_pre_major_inference_policy_applied() {
    use crate::config::resolve::PreMajorInferencePolicy;
    use std::sync::atomic::AtomicBool;

    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    init_repo(root);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial commit"]);

    // Write a callisto.toml with pre_major_inference = "conservative" for pkg-a.
    // The [[package]] section requires a `match` field (pattern to match package names).
    std::fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"pkg-a\"\npre-major-inference = \"conservative\"\n",
    )
    .unwrap();

    let pkg_id = PackageId::parse("pkg-a").unwrap();
    let graph = SinglePackageGraph {
        pkg: make_pkg(pkg_id.clone()),
    };
    let cfg = crate::config::load(root).unwrap();
    let runner = RealGitRunner;
    let git = GitAccess::new(root, &runner);
    let tags = crate::tags::TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();
    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_id.clone(), Version::semver(0, 1, 0));

    // An inference impl that records whether it received a non-OFF policy.
    struct PolicyCapturingInference {
        saw_non_off: AtomicBool,
    }
    impl SeverityInference for PolicyCapturingInference {
        fn infer(
            &self,
            _pkg: &Package,
            _git: &GitAccess<'_>,
            window: InferenceWindowSpec<'_>,
        ) -> Result<Option<InferenceOutcome>, GraphError> {
            if window.policy != PreMajorInferencePolicy::Off {
                self.saw_non_off.store(true, Ordering::SeqCst);
            }
            Ok(None)
        }
    }

    let capturing = PolicyCapturingInference {
        saw_non_off: AtomicBool::new(false),
    };
    aggregate(
        &graph,
        &identity_for(&graph.pkg),
        &cfg,
        &git,
        &tags,
        &base_versions,
        None,
        &capturing,
    )
    .unwrap();

    assert!(
        capturing.saw_non_off.load(Ordering::SeqCst),
        "aggregate() must pass the per-package pre_major_inference policy from config.packages \
             into InferenceWindowSpec; received OFF even though callisto.toml sets conservative"
    );
}

/// Prefixed rule (npm:pkg, OFF policy) must win over Bare rule (pkg, conservative)
/// even when the Bare rule is declared first in callisto.toml.
/// With single-pass lookup: the Bare rule (declared first) wins -> conservative applied.
/// With resolve_package_config (two-pass): the Prefixed rule wins -> OFF applied.
///
/// Two AtomicBool flags:
/// - invoked: true if infer() was called at all (proves the package is pre-1.0 and
///   pre-major-inference was consulted; distinguishes OFF-applied from never-called).
/// - saw_non_off: true if infer() received a non-OFF policy (single-pass failure mode).
#[test]
fn test_pre_major_inference_prefixed_beats_bare_when_bare_declared_first() {
    use crate::config::resolve::PreMajorInferencePolicy;
    use std::sync::atomic::AtomicBool;

    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    init_repo(root);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial commit"]);

    // Bare rule declared FIRST in TOML: match = "pkg", pre-major-inference = "conservative"
    // Prefixed rule declared SECOND: match = "npm:pkg", pre-major-inference = "off"
    // Single-pass find() returns pkg (Bare, declared first) -> conservative applied.
    // Two-pass resolve_package_config: pass 1 finds npm:pkg (Prefixed) -> OFF applied.
    // The package's own manifest is npm (`make_pkg`'s ecosystem-derived-from-id, via a
    // Prefixed id here) so the `npm:pkg` rule is actually ecosystem-eligible, not just
    // name-eligible -- this test is about two-pass declaration-order precedence, not
    // about a rule applying to a package that was never really in that ecosystem.
    std::fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"pkg\"\npre-major-inference = \"conservative\"\n\n[[package]]\nmatch = \"npm:pkg\"\npre-major-inference = \"off\"\n",
        )
        .unwrap();

    let pkg_id = PackageId::Prefixed {
        ecosystem: Ecosystem::Npm,
        name: "pkg".to_string(),
    };
    let graph = SinglePackageGraph {
        pkg: make_pkg(pkg_id.clone()),
    };
    let cfg = crate::config::load(root).unwrap();
    let runner = RealGitRunner;
    let git = GitAccess::new(root, &runner);
    let tags = crate::tags::TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();
    let mut base_versions = BTreeMap::new();
    // Version 0.1.0 (pre-1.0) ensures pre-major-inference is consulted.
    base_versions.insert(pkg_id.clone(), Version::semver(0, 1, 0));

    struct PolicyCapturingInference {
        invoked: AtomicBool,
        saw_non_off: AtomicBool,
    }
    impl SeverityInference for PolicyCapturingInference {
        fn infer(
            &self,
            _pkg: &Package,
            _git: &GitAccess<'_>,
            window: InferenceWindowSpec<'_>,
        ) -> Result<Option<InferenceOutcome>, GraphError> {
            // Set invoked before any policy check so we can distinguish
            // OFF-policy-applied from never-called.
            self.invoked.store(true, Ordering::SeqCst);
            if window.policy != PreMajorInferencePolicy::Off {
                self.saw_non_off.store(true, Ordering::SeqCst);
            }
            Ok(None)
        }
    }

    let capturing = PolicyCapturingInference {
        invoked: AtomicBool::new(false),
        saw_non_off: AtomicBool::new(false),
    };
    aggregate(
        &graph,
        &identity_for(&graph.pkg),
        &cfg,
        &git,
        &tags,
        &base_versions,
        None,
        &capturing,
    )
    .unwrap();

    assert!(
        capturing.invoked.load(Ordering::SeqCst),
        "inference was never invoked; fixture is wrong and cannot distinguish OFF policy \
             from no invocation. Ensure Version::semver(0, 1, 0) is in base_versions so the \
             package is pre-1.0 and pre-major-inference is consulted."
    );
    assert!(
        !capturing.saw_non_off.load(Ordering::SeqCst),
        "expected OFF policy from Prefixed rule (npm:pkg); Bare rule (conservative) was \
             applied instead. Two-pass specificity is required: Prefixed rules must win over \
             Bare rules regardless of declaration order."
    );
}

/// A changeset matched for the first time in pre mode is recorded into `new_pre_changesets`, not `consumed`.
#[test]
fn test_pre_mode_first_match_is_recorded_not_consumed() {
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();
    init_repo(root);

    let cs_dir = root.join(".changeset");
    std::fs::create_dir_all(&cs_dir).unwrap();
    std::fs::write(
        cs_dir.join("cool-thing.md"),
        "---\npkg-a: minor\n---\n\nAdds a thing.\n",
    )
    .unwrap();
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial commit"]);

    let pkg_id = PackageId::parse("pkg-a").unwrap();
    let graph = SinglePackageGraph {
        pkg: make_pkg(pkg_id.clone()),
    };
    let cfg = crate::config::load(root).unwrap();
    let runner = RealGitRunner;
    let git = GitAccess::new(root, &runner);
    let tags = crate::tags::TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();
    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_id.clone(), Version::semver(0, 1, 0));

    let mut initial_versions = indexmap::IndexMap::new();
    initial_versions.insert("pkg-a".to_string(), Version::semver(0, 1, 0));
    let pre_state = callisto_model::format::PreState {
        mode: callisto_model::format::PreMode::Pre,
        tag: "beta".to_string(),
        initial_versions,
        changesets: Vec::new(),
    };

    let inference = crate::infer::NoInference;
    let agg = aggregate(
        &graph,
        &identity_for(&graph.pkg),
        &cfg,
        &git,
        &tags,
        &base_versions,
        Some(&pre_state),
        &inference,
    )
    .unwrap();

    assert_eq!(
        agg.new_pre_changesets,
        vec!["cool-thing".to_string()],
        "a changeset matched for the first time in pre mode must be recorded as new"
    );
    assert!(
        agg.consumed.is_empty(),
        "pre mode must never add a changeset to `consumed` (would delete it from disk); got: {:?}",
        agg.consumed
    );
    assert_eq!(
        agg.changelog_inputs.get(&pkg_id).map(|i| i.entries.len()),
        Some(1),
        "a newly-seen pre-mode changeset must produce exactly one changelog entry"
    );
}

/// A changeset id already in `pre.json` counts toward severity on rerun but must not add a second changelog entry.
#[test]
fn test_pre_mode_already_recorded_changeset_counts_severity_but_not_changelog() {
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();
    init_repo(root);

    let cs_dir = root.join(".changeset");
    std::fs::create_dir_all(&cs_dir).unwrap();
    std::fs::write(
        cs_dir.join("cool-thing.md"),
        "---\npkg-a: minor\n---\n\nAdds a thing.\n",
    )
    .unwrap();
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial commit"]);

    let pkg_id = PackageId::parse("pkg-a").unwrap();
    let graph = SinglePackageGraph {
        pkg: make_pkg(pkg_id.clone()),
    };
    let cfg = crate::config::load(root).unwrap();
    let runner = RealGitRunner;
    let git = GitAccess::new(root, &runner);
    let tags = crate::tags::TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();
    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_id.clone(), Version::semver(0, 1, 0));

    let mut initial_versions = indexmap::IndexMap::new();
    initial_versions.insert("pkg-a".to_string(), Version::semver(0, 1, 0));
    let pre_state = callisto_model::format::PreState {
        mode: callisto_model::format::PreMode::Pre,
        tag: "beta".to_string(),
        initial_versions,
        // Simulates a prior `version` run already having recorded this id.
        changesets: vec!["cool-thing".to_string()],
    };

    let inference = crate::infer::NoInference;
    let agg = aggregate(
        &graph,
        &identity_for(&graph.pkg),
        &cfg,
        &git,
        &tags,
        &base_versions,
        Some(&pre_state),
        &inference,
    )
    .unwrap();

    assert!(
        agg.new_pre_changesets.is_empty(),
        "an already-recorded pre-mode changeset must not be reported as new; got: {:?}",
        agg.new_pre_changesets
    );
    assert!(
        agg.consumed.is_empty(),
        "pre mode must never delete a changeset from disk, recorded or not; got: {:?}",
        agg.consumed
    );
    assert_eq!(
        agg.severities.get(&pkg_id),
        Some(&Severity::Minor),
        "an already-recorded changeset must still count toward severity against initialVersions"
    );
    assert!(
        !agg.changelog_inputs.contains_key(&pkg_id),
        "an already-recorded pre-mode changeset must not produce a new changelog entry on rerun; \
             got: {:?}",
        agg.changelog_inputs.get(&pkg_id)
    );
    assert!(
        agg.pre_mode_has_active_changeset,
        "an already-recorded changeset still actively drives this run (severity + prerelease \
             counter), so pre_mode_has_active_changeset must be true, not just new_pre_changesets"
    );
}

/// With no changeset on disk in pre mode, the run has no active changeset, not just an empty `new_pre_changesets`.
#[test]
fn test_pre_mode_no_changesets_at_all_has_no_active_changeset() {
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();
    init_repo(root);
    std::fs::write(root.join("README.md"), "hello\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "initial commit"]);

    let pkg_id = PackageId::parse("pkg-a").unwrap();
    let graph = SinglePackageGraph {
        pkg: make_pkg(pkg_id.clone()),
    };
    let cfg = crate::config::load(root).unwrap();
    let runner = RealGitRunner;
    let git = GitAccess::new(root, &runner);
    let tags = crate::tags::TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();
    let mut base_versions = BTreeMap::new();
    base_versions.insert(pkg_id, Version::semver(0, 1, 0));

    let mut initial_versions = indexmap::IndexMap::new();
    initial_versions.insert("pkg-a".to_string(), Version::semver(0, 1, 0));
    let pre_state = callisto_model::format::PreState {
        mode: callisto_model::format::PreMode::Pre,
        tag: "beta".to_string(),
        initial_versions,
        changesets: Vec::new(),
    };

    let inference = crate::infer::NoInference;
    let agg = aggregate(
        &graph,
        &identity_for(&graph.pkg),
        &cfg,
        &git,
        &tags,
        &base_versions,
        Some(&pre_state),
        &inference,
    )
    .unwrap();

    assert!(!agg.pre_mode_has_active_changeset);
    assert!(agg.new_pre_changesets.is_empty());
}
