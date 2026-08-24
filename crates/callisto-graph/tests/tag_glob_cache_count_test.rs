//! Regression test for `TagIndex::build`'s per-template glob-compile
//! caching. Isolated in its own integration-test binary because
//! `callisto_graph::tags::glob_compile_count` is a process-global counter
//! that other, non-`#[serial]` tests in a shared binary would pollute (see
//! `tests/apply_persist_open_count_test.rs` for the precedent this file
//! follows).

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use callisto_graph::resolver::DependencyResolver;
use callisto_graph::tags::TagIndex;
use callisto_model::{
    CommandError, CommandOutput, CommandRunner, DepEdge, ManifestDecl, ManifestFormat, ManifestRole, Package,
    PackageId, TagTemplate,
};
use callisto_vcs::GitAccess;
use serial_test::serial;

fn make_pkg(name: &str, tag_template: Option<TagTemplate>) -> Package {
    let manifest = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    Package {
        id: PackageId::parse(name).unwrap(),
        manifests: vec![manifest],
        changelog: None,
        release_trigger: callisto_model::ReleaseTrigger::Changeset,
        publish_to: Vec::new(),
        tag_template,
    }
}

struct FixedGraph {
    pkgs: Vec<Package>,
}

impl DependencyResolver for FixedGraph {
    fn packages(&self) -> impl Iterator<Item = &Package> {
        self.pkgs.iter()
    }

    fn dependencies_of(&self, _id: &PackageId) -> impl Iterator<Item = &DepEdge> {
        std::iter::empty()
    }

    fn dependents_of(&self, _id: &PackageId) -> impl Iterator<Item = &DepEdge> {
        std::iter::empty()
    }
}

/// A `CommandRunner` double that never touches a real `git` binary: it
/// answers `git tag --list` with a canned tag list.
struct FakeGitTagRunner {
    calls: AtomicUsize,
    tags: Vec<String>,
}

impl FakeGitTagRunner {
    fn new(tags: Vec<String>) -> Self {
        FakeGitTagRunner {
            calls: AtomicUsize::new(0),
            tags,
        }
    }
}

impl CommandRunner for FakeGitTagRunner {
    fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
        assert_eq!(program, "git");
        assert_eq!(args, ["tag", "--list"]);
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(CommandOutput {
            exit_code: Some(0),
            stdout: self.tags.join("\n"),
            stderr: String::new(),
        })
    }
}

/// A directory guaranteed not to sit inside any Git repository, forcing
/// `GitAccess::discover` through the `CommandRunner` fallback path (mirrors
/// `callisto-graph`'s own `tags.rs` unit-test fixture of the same name).
fn non_repo_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        callisto_vcs::GitRepository::discover(dir.path()).is_err(),
        "test fixture must not be discoverable as a Git repo"
    );
    dir
}

/// Spec: `TagIndex::build` must compile/scan a given tag-template glob at
/// most once per *distinct* template string, not once per package -- a
/// `[[package-set]]` rule with a fixed (non-`{name}`) `tag-template` applies
/// the identical template to every matching package, so N packages sharing
/// one explicit template must cost one glob compile, not N.
#[test]
#[serial]
fn tag_index_build_compiles_shared_glob_template_once() {
    let dir = non_repo_dir();
    let runner = FakeGitTagRunner::new(vec!["release@1.0.0".to_string()]);

    let shared_template = TagTemplate::parse("release@{version}").unwrap();
    let pkgs = vec![
        make_pkg("pkg-a", Some(shared_template.clone())),
        make_pkg("pkg-b", Some(shared_template.clone())),
        make_pkg("pkg-c", Some(shared_template)),
    ];
    let graph = FixedGraph { pkgs };
    let cfg = callisto_graph::config::load(dir.path()).unwrap();
    let git = GitAccess::discover(dir.path(), &runner);

    callisto_graph::tags::reset_glob_compile_count();
    TagIndex::build(&git, &graph, &cfg).expect("TagIndex::build must succeed");

    assert_eq!(
        callisto_graph::tags::glob_compile_count(),
        1,
        "TagIndex::build must compile a shared tag-template glob once for all 3 packages, not once per package"
    );
}

/// Distinct per-package templates (the default, name-derived template) must
/// still compile once each -- the cache must key on the template string, not
/// collapse every package onto one entry.
#[test]
#[serial]
fn tag_index_build_compiles_distinct_glob_templates_separately() {
    let dir = non_repo_dir();
    let runner = FakeGitTagRunner::new(vec!["pkg-a@1.0.0".to_string(), "pkg-b@1.0.0".to_string()]);

    let pkgs = vec![make_pkg("pkg-a", None), make_pkg("pkg-b", None)];
    let graph = FixedGraph { pkgs };
    let cfg = callisto_graph::config::load(dir.path()).unwrap();
    let git = GitAccess::discover(dir.path(), &runner);

    callisto_graph::tags::reset_glob_compile_count();
    TagIndex::build(&git, &graph, &cfg).expect("TagIndex::build must succeed");

    assert_eq!(
        callisto_graph::tags::glob_compile_count(),
        2,
        "2 packages with distinct default templates must each compile their own glob"
    );
}
