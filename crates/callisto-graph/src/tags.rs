use std::collections::BTreeMap;
use std::path::Path;

use callisto_model::{
    select_last_tag, CommandRunner, CommitSha, Diagnostic, LastTag, LastTagSelection, PackageId,
    TagTemplate, VersionGrammar,
};
use callisto_vcs::{GitAccess, GitDataSource};

use crate::config::ResolvedConfig;
use crate::error::GraphError;
use crate::resolver::DependencyResolver;

/// Fetches the full, unfiltered list of every tag name in the repository at
/// `root`, via [`GitAccess`] (native gix, falling back to a `CommandRunner`-
/// shelled `git tag --list` when gix is unavailable -- most notably on
/// `wasm32`).
///
/// Deliberately fetches with no glob pattern (the full tag list): callers
/// apply their own glob filtering afterwards via [`matching_tags`], using
/// the exact same `globset` matcher both `GitDataSource` backends use
/// internally. This guarantees identical tag-selection semantics
/// regardless of which backend sourced the raw list, and lets the fetch be
/// batched once across every package (see [`TagIndex::build`]) instead of
/// once per package -- important on `wasm32`, where each `CommandRunner`
/// call is a full Extism guest<->host round-trip.
fn fetch_all_tags<R: CommandRunner>(runner: &R, root: &Path) -> Result<Vec<String>, GraphError> {
    let git = GitAccess::discover(root, runner);
    let tags = git.list_tags(None)?;
    Ok(tags.into_iter().map(|t| t.0).collect())
}

/// Filters `all_tags` down to those matching `template`'s glob.
///
/// Uses the same `globset::Glob`-based matching
/// `callisto_vcs::GitRepository::list_tags` applies internally, kept in
/// sync deliberately so tag selection is byte-identical whether `all_tags`
/// was sourced via gix or the `CommandRunner` fallback in
/// [`fetch_all_tags`]. This includes error behavior: a `template.glob()`
/// that fails to compile is surfaced as `Err(GraphError::Vcs(VcsError::
/// InvalidGlob))`, matching every tag being the unsafe alternative -- a
/// malformed tag template must never silently make "last tag" resolution
/// pick an unrelated package's tag.
fn matching_tags<'a>(
    all_tags: &'a [String],
    template: &TagTemplate,
) -> Result<Vec<&'a str>, GraphError> {
    let glob = template.glob();
    let matcher = globset::Glob::new(&glob)
        .map(|g| g.compile_matcher())
        .map_err(|e| {
            GraphError::Vcs(callisto_vcs::VcsError::InvalidGlob {
                pattern: glob.clone(),
                message: e.to_string(),
            })
        })?;

    Ok(all_tags
        .iter()
        .filter(|t| matcher.is_match(t.as_str()))
        .map(|s| s.as_str())
        .collect())
}

/// Selects the highest-versioned tag matching `template` out of a full tag
/// list previously obtained via [`fetch_all_tags`].
fn select_from_tags(
    all_tags: &[String],
    template: &TagTemplate,
    grammar: VersionGrammar,
) -> Result<LastTagSelection, GraphError> {
    let candidates = matching_tags(all_tags, template)?;
    select_last_tag(template, grammar, candidates).map_err(GraphError::from)
}

/// Resolves the last release tag matching a single package's `template`.
///
/// Kept for API compatibility with existing callers, and safe to use
/// standalone. Note that this re-fetches the full tag list (gix discovery
/// or a `CommandRunner` round-trip) on every call; callers resolving tags
/// for many packages at once -- notably [`TagIndex::build`] -- fetch the
/// list once and reuse it across packages rather than calling this
/// function in a loop.
pub fn last_tag_for<R: CommandRunner>(
    runner: &R,
    root: &Path,
    template: &TagTemplate,
    grammar: VersionGrammar,
) -> Result<LastTagSelection, GraphError> {
    let all_tags = fetch_all_tags(runner, root)?;
    select_from_tags(&all_tags, template, grammar)
}

pub struct TagIndex {
    last: BTreeMap<PackageId, Option<LastTag>>,
    templates: BTreeMap<PackageId, TagTemplate>,
    pre_cursor: BTreeMap<PackageId, Option<CommitSha>>,
    pub diagnostics: Vec<Diagnostic>,
}

impl TagIndex {
    pub fn build<R: CommandRunner, D: DependencyResolver>(
        runner: &R,
        root: &Path,
        graph: &D,
        _cfg: &ResolvedConfig,
    ) -> Result<Self, GraphError> {
        let mut last = BTreeMap::new();
        let mut templates = BTreeMap::new();
        let mut pre_cursor = BTreeMap::new();
        let diagnostics = Vec::new();

        // Fetch the raw tag list exactly once for the whole build, not once
        // per package -- see `fetch_all_tags` for why (avoids N gix
        // discoveries / N host round-trips for N packages).
        let all_tags = fetch_all_tags(runner, root)?;

        for pkg in graph.packages() {
            let default_tmpl =
                TagTemplate::parse(&format!("{}@{{version}}", pkg.id.display_name()))?;
            let tmpl = pkg.tag_template.clone().unwrap_or(default_tmpl);
            let sel = select_from_tags(&all_tags, &tmpl, pkg.version_grammar()?)?;
            last.insert(pkg.id.clone(), sel.chosen);
            templates.insert(pkg.id.clone(), tmpl);
            pre_cursor.insert(pkg.id.clone(), None);
        }

        Ok(TagIndex {
            last,
            templates,
            pre_cursor,
            diagnostics,
        })
    }

    pub fn last_tag(&self, id: &PackageId) -> Option<&LastTag> {
        self.last.get(id).and_then(|opt| opt.as_ref())
    }

    pub fn template(&self, id: &PackageId) -> &TagTemplate {
        &self.templates[id]
    }

    pub fn pre_cursor(&self, id: &PackageId) -> Option<&CommitSha> {
        self.pre_cursor.get(id).and_then(|opt| opt.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use callisto_model::{
        CommandError, CommandOutput, DepEdge, ManifestDecl, ManifestFormat, ManifestRole, Package,
    };

    fn make_pkg(name: &str) -> Package {
        let manifest = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        Package {
            id: PackageId::parse(name).unwrap(),
            manifests: vec![manifest],
            changelog: None,
            release_trigger: callisto_model::ReleaseTrigger::Changeset,
            publish_to: Vec::new(),
            tag_template: None,
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
    /// answers `git tag --list` with a canned tag list and counts every
    /// invocation. Used both to prove `last_tag_for`/`TagIndex::build`
    /// succeed with gix unavailable (mirroring the wasm32 code path, where
    /// `GitRepository::discover` always fails), and to count how many
    /// `CommandRunner` round-trips a `TagIndex::build` call costs.
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
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &Path,
        ) -> Result<CommandOutput, CommandError> {
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

    /// A directory that is guaranteed not to sit inside any Git repository,
    /// so `callisto_vcs::GitRepository::discover` fails exactly the way it
    /// unconditionally does on `wasm32` -- this is the native-testable
    /// stand-in for "gix is unavailable" the spec calls for, forcing every
    /// path under test through the `CommandRunner` fallback.
    fn non_repo_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            callisto_vcs::GitRepository::discover(dir.path()).is_err(),
            "test fixture must not be discoverable as a Git repo"
        );
        dir
    }

    /// Spec: `last_tag_for` must not hard-fail when gix is unavailable
    /// (reproduces the wasm32 crash natively) -- it must fall back to the
    /// `CommandRunner` and still select the right tag.
    #[test]
    fn test_last_tag_for_succeeds_without_gix() {
        let dir = non_repo_dir();
        let runner = FakeGitTagRunner::new(vec![
            "pkg-a@1.0.0".to_string(),
            "pkg-a@1.2.0".to_string(),
            "unrelated-tag".to_string(),
        ]);
        let tmpl = TagTemplate::parse("pkg-a@{version}").unwrap();

        let sel = last_tag_for(&runner, dir.path(), &tmpl, VersionGrammar::SemVer)
            .expect("last_tag_for must succeed via the CommandRunner fallback when gix cannot discover a repo");

        assert_eq!(
            sel.chosen.map(|t| t.version.render().to_string()),
            Some("1.2.0".to_string())
        );
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    }

    /// Spec: `TagIndex::build` must not hard-fail when gix is unavailable
    /// either -- it drives the same fallback for every package in the
    /// graph.
    #[test]
    fn test_tag_index_build_succeeds_without_gix() {
        let dir = non_repo_dir();
        let runner = FakeGitTagRunner::new(vec!["pkg-a@2.0.0".to_string()]);
        let graph = FixedGraph {
            pkgs: vec![make_pkg("pkg-a")],
        };
        let cfg = crate::config::load(dir.path()).unwrap();

        let tags = TagIndex::build(&runner, dir.path(), &graph, &cfg)
            .expect("TagIndex::build must succeed via the CommandRunner fallback when gix cannot discover a repo");

        let pkg_id = PackageId::parse("pkg-a").unwrap();
        assert_eq!(
            tags.last_tag(&pkg_id)
                .map(|t| t.version.render().to_string()),
            Some("2.0.0".to_string())
        );
    }

    /// Spec: `TagIndex::build` must fetch the raw tag list exactly once per
    /// build, not once per package -- each `CommandRunner` round-trip is a
    /// full Extism guest<->host context switch on the wasm32/moon path, so
    /// N packages must not cost N round-trips.
    #[test]
    fn test_tag_index_build_batches_tag_fetch_across_packages() {
        let dir = non_repo_dir();
        let runner = FakeGitTagRunner::new(vec![
            "pkg-a@1.0.0".to_string(),
            "pkg-b@1.0.0".to_string(),
            "pkg-c@1.0.0".to_string(),
        ]);
        let graph = FixedGraph {
            pkgs: vec![make_pkg("pkg-a"), make_pkg("pkg-b"), make_pkg("pkg-c")],
        };
        let cfg = crate::config::load(dir.path()).unwrap();

        let tags = TagIndex::build(&runner, dir.path(), &graph, &cfg).unwrap();

        for name in ["pkg-a", "pkg-b", "pkg-c"] {
            let id = PackageId::parse(name).unwrap();
            assert!(tags.last_tag(&id).is_some(), "{name} should have a tag");
        }

        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            1,
            "TagIndex::build must fetch the tag list once for all 3 packages, not once per package"
        );
    }

    /// Spec: on a real, gix-discoverable repo, `matching_tags` must select
    /// exactly the tags `callisto_vcs::GitRepository::list_tags`'s own
    /// glob-based filtering would -- proving the two paths share identical
    /// selection semantics.
    #[test]
    fn test_matching_tags_mirrors_globset_semantics() {
        let all = vec![
            "pkg-a@1.0.0".to_string(),
            "pkg-ab@1.0.0".to_string(),
            "pkg-a@2.0.0-beta".to_string(),
            "other".to_string(),
        ];
        let tmpl = TagTemplate::parse("pkg-a@{version}").unwrap();
        let matched = matching_tags(&all, &tmpl).unwrap();
        assert_eq!(matched, vec!["pkg-a@1.0.0", "pkg-a@2.0.0-beta"]);
    }

    /// Spec: a `TagTemplate` whose glob fails to compile must make
    /// `matching_tags` return `Err`, not silently disable filtering and
    /// match every tag in the repo. `TagTemplate::parse` only rejects
    /// `*`/`?`/`[`/`]` in literal text -- it does not reject unbalanced
    /// `{`/`}`, so a template like `pkg-a@{version}{oops` parses
    /// successfully but renders the glob `pkg-a@*{oops`, which `globset`
    /// cannot compile (unclosed alternate group). This is a real
    /// correctness risk for release tagging: silently matching every tag
    /// could make "last tag" resolution pick an unrelated package's tag.
    #[test]
    fn test_matching_tags_rejects_malformed_glob_instead_of_matching_everything() {
        let all = vec![
            "pkg-a@1.0.0".to_string(),
            "totally-unrelated-tag".to_string(),
            "another-unrelated-tag".to_string(),
        ];
        let tmpl = TagTemplate::parse("pkg-a@{version}{oops").unwrap();
        // Sanity-check the premise: this template's glob really is
        // uncompilable.
        assert!(globset::Glob::new(&tmpl.glob()).is_err());

        let result = matching_tags(&all, &tmpl);

        assert!(
            matches!(
                result,
                Err(GraphError::Vcs(callisto_vcs::VcsError::InvalidGlob { .. }))
            ),
            "malformed glob must be surfaced as Err, not silently match every tag; got {result:?}"
        );
    }

    /// Spec: the malformed-glob error must propagate all the way up
    /// through `select_from_tags`/`last_tag_for`, not just the internal
    /// `matching_tags` helper.
    #[test]
    fn test_last_tag_for_propagates_malformed_glob_error() {
        let dir = non_repo_dir();
        let runner = FakeGitTagRunner::new(vec!["pkg-a@1.0.0".to_string()]);
        let tmpl = TagTemplate::parse("pkg-a@{version}{oops").unwrap();

        let result = last_tag_for(&runner, dir.path(), &tmpl, VersionGrammar::SemVer);

        assert!(
            matches!(
                result,
                Err(GraphError::Vcs(callisto_vcs::VcsError::InvalidGlob { .. }))
            ),
            "last_tag_for must propagate the malformed-glob error, got {result:?}"
        );
    }

    /// Spec: `TagIndex::build`/`last_tag_for` against a repo with zero tags
    /// at all must resolve cleanly to `None`, never panic.
    #[test]
    fn test_last_tag_for_with_zero_tags_returns_none() {
        let dir = non_repo_dir();
        let runner = FakeGitTagRunner::new(vec![]);
        let tmpl = TagTemplate::parse("pkg-a@{version}").unwrap();

        let sel = last_tag_for(&runner, dir.path(), &tmpl, VersionGrammar::SemVer).unwrap();

        assert!(sel.chosen.is_none());
    }

    #[test]
    fn test_tag_index_build_with_zero_tags_returns_none_for_every_package() {
        let dir = non_repo_dir();
        let runner = FakeGitTagRunner::new(vec![]);
        let graph = FixedGraph {
            pkgs: vec![make_pkg("pkg-a"), make_pkg("pkg-b")],
        };
        let cfg = crate::config::load(dir.path()).unwrap();

        let tags = TagIndex::build(&runner, dir.path(), &graph, &cfg).unwrap();

        for name in ["pkg-a", "pkg-b"] {
            let id = PackageId::parse(name).unwrap();
            assert!(
                tags.last_tag(&id).is_none(),
                "{name} should have no last tag in a zero-tag repo"
            );
        }
    }

    /// Spec: `TagIndex::build` across multiple packages whose glob patterns
    /// select from an overlapping/shared tag namespace must resolve each
    /// package to its own highest-versioned tag, not cross-contaminate.
    /// `pkg-a` and `pkg-ab` share the `pkg-a` prefix, so this also exercises
    /// that `matching_tags`'s glob matching (not a naive `starts_with`)
    /// keeps them apart.
    #[test]
    fn test_tag_index_build_multiple_packages_overlapping_tag_prefixes() {
        let dir = non_repo_dir();
        let runner = FakeGitTagRunner::new(vec![
            "pkg-a@1.0.0".to_string(),
            "pkg-a@1.5.0".to_string(),
            "pkg-ab@9.0.0".to_string(),
            "pkg-ab@9.1.0".to_string(),
        ]);
        let graph = FixedGraph {
            pkgs: vec![make_pkg("pkg-a"), make_pkg("pkg-ab")],
        };
        let cfg = crate::config::load(dir.path()).unwrap();

        let tags = TagIndex::build(&runner, dir.path(), &graph, &cfg).unwrap();

        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let pkg_ab = PackageId::parse("pkg-ab").unwrap();
        assert_eq!(
            tags.last_tag(&pkg_a)
                .map(|t| t.version.render().to_string()),
            Some("1.5.0".to_string())
        );
        assert_eq!(
            tags.last_tag(&pkg_ab)
                .map(|t| t.version.render().to_string()),
            Some("9.1.0".to_string())
        );
    }

    /// A `CommandRunner` double that always fails, standing in for a real
    /// `git` binary missing/erroring on the `CommandRunner` fallback path
    /// (used when gix is unavailable).
    struct FailingRunner;

    impl CommandRunner for FailingRunner {
        fn run(
            &self,
            _program: &str,
            _args: &[&str],
            _cwd: &Path,
        ) -> Result<CommandOutput, CommandError> {
            Err(CommandError::NotFound {
                program: "git".to_string(),
            })
        }
    }

    /// Spec: when gix is unavailable and the `CommandRunner` fallback
    /// itself returns `Err`, `fetch_all_tags` (exercised via
    /// `last_tag_for`) must propagate that error rather than panicking or
    /// silently swallowing it into an empty tag list. Now routed through
    /// `GitAccess`/`GitDataSource`, so the error arrives wrapped as
    /// `GraphError::Vcs(VcsError::Command(_))` rather than the direct
    /// `GraphError::Command(_)` the old hand-rolled shell-out produced --
    /// same propagation guarantee, new (centralized) shape.
    #[test]
    fn test_last_tag_for_propagates_command_runner_error() {
        let dir = non_repo_dir();
        let runner = FailingRunner;
        let tmpl = TagTemplate::parse("pkg-a@{version}").unwrap();

        let result = last_tag_for(&runner, dir.path(), &tmpl, VersionGrammar::SemVer);

        assert!(
            matches!(
                result,
                Err(GraphError::Vcs(callisto_vcs::VcsError::Command(_)))
            ),
            "last_tag_for must propagate the CommandRunner error, got {result:?}"
        );
    }

    /// Spec: `TagIndex::build` must use `pkg.tag_template` when it is set
    /// instead of always defaulting to `{name}@{version}`. A package with
    /// `tag_template: Some(TagTemplate::parse("v{version}"))` must resolve
    /// tags like `"v1.2.3"`, and a package with `tag_template: None` must
    /// still fall back to the default `{name}@{version}` pattern.
    #[test]
    fn tag_index_uses_custom_tag_template_when_set() {
        let dir = non_repo_dir();
        let runner =
            FakeGitTagRunner::new(vec!["v1.2.3".to_string(), "pkg-default@4.5.6".to_string()]);

        let manifest = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();

        let custom_pkg = Package {
            id: PackageId::parse("custom-pkg").unwrap(),
            manifests: vec![manifest.clone()],
            changelog: None,
            release_trigger: callisto_model::ReleaseTrigger::Changeset,
            publish_to: Vec::new(),
            tag_template: Some(TagTemplate::parse("v{version}").unwrap()),
        };
        let default_pkg = Package {
            id: PackageId::parse("pkg-default").unwrap(),
            manifests: vec![manifest],
            changelog: None,
            release_trigger: callisto_model::ReleaseTrigger::Changeset,
            publish_to: Vec::new(),
            tag_template: None,
        };

        let graph = FixedGraph {
            pkgs: vec![custom_pkg, default_pkg],
        };
        let cfg = crate::config::load(dir.path()).unwrap();

        let tags = TagIndex::build(&runner, dir.path(), &graph, &cfg)
            .expect("TagIndex::build must succeed");

        let custom_id = PackageId::parse("custom-pkg").unwrap();
        let default_id = PackageId::parse("pkg-default").unwrap();

        assert_eq!(
            tags.last_tag(&custom_id)
                .map(|t| t.version.render().to_string()),
            Some("1.2.3".to_string()),
            "package with custom tag_template 'v{{version}}' must resolve 'v1.2.3'"
        );
        assert_eq!(
            tags.last_tag(&default_id)
                .map(|t| t.version.render().to_string()),
            Some("4.5.6".to_string()),
            "package with no tag_template must fall back to 'pkg-default@{{version}}'"
        );
    }

    /// Spec: same as above, but for `TagIndex::build` -- a `CommandRunner`
    /// failure on the fallback path must propagate up through the whole
    /// build, not be swallowed per-package.
    #[test]
    fn test_tag_index_build_propagates_command_runner_error() {
        let dir = non_repo_dir();
        let runner = FailingRunner;
        let graph = FixedGraph {
            pkgs: vec![make_pkg("pkg-a")],
        };
        let cfg = crate::config::load(dir.path()).unwrap();

        let is_command_err = matches!(
            TagIndex::build(&runner, dir.path(), &graph, &cfg),
            Err(GraphError::Vcs(callisto_vcs::VcsError::Command(_)))
        );

        assert!(
            is_command_err,
            "TagIndex::build must propagate the CommandRunner error as GraphError::Command"
        );
    }
}
