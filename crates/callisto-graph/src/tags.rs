use std::collections::BTreeMap;

use callisto_model::vcs::GitAccess;
use callisto_model::{
    select_last_tag, CommitSha, Diagnostic, LastTag, LastTagSelection, PackageId, TagTemplate, VersionGrammar,
};

use crate::config::ResolvedConfig;
use crate::error::GraphError;
use crate::resolver::DependencyResolver;

/// Fetches the full, unfiltered list of every tag name in the repository.
///
/// Deliberately fetches with no glob pattern: callers filter afterwards via
/// [`matching_tags`], so the fetch batches once across every package (see
/// [`TagIndex::build`]) instead of one `git` spawn per package.
pub(crate) fn fetch_all_tags(git: &GitAccess<'_>) -> Result<Vec<String>, GraphError> {
    let tags = git.list_tags(None)?;
    Ok(tags.into_iter().map(|t| t.as_str().to_string()).collect())
}

/// Filters `all_tags` down to those matching `template`'s glob.
///
/// Compiles the glob via [`callisto_model::vcs::compile_tag_glob`] -- the same
/// helper `GitAccess::list_tags` uses -- so tag selection is identical
/// either way. Includes error behavior: a
/// `template.glob()` that fails to compile surfaces as
/// `Err(GraphError::Vcs(VcsError::InvalidGlob))` -- matching every tag is
/// the unsafe alternative, since a malformed template must never silently
/// make "last tag" resolution pick an unrelated package's tag.
fn matching_tags<'a>(all_tags: &'a [String], template: &TagTemplate) -> Result<Vec<&'a str>, GraphError> {
    let glob = template.glob();
    GLOB_COMPILE_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let matcher = callisto_model::vcs::compile_tag_glob(&glob).map_err(GraphError::Vcs)?;

    Ok(all_tags
        .iter()
        .filter(|t| matcher.is_match(t.as_str()))
        .map(|s| s.as_str())
        .collect())
}

/// Test-observability counter: total number of times [`matching_tags`] has
/// compiled a fresh `globset::Glob`. Production code never reads this; it
/// exists so tests can assert `TagIndex::build` compiles/scans at most once
/// per *distinct* tag-template glob, not once per package -- a
/// `[[package-set]]` rule with a fixed (non-`{name}`) `tag-template` can
/// apply the identical template string to many packages at once.
static GLOB_COMPILE_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Resets the internal glob-compile call counter to zero. Intended for use in test setup.
///
/// Process-global, like `callisto_manifests::reset_open_call_count` -- exact
/// counts must be asserted from an isolated integration-test binary under
/// `#[serial]`, not an inline unit test sharing this crate's `--lib`
/// process with other, non-serial tests (see
/// `tests/tag_glob_cache_count_test.rs`).
pub fn reset_glob_compile_count() {
    GLOB_COMPILE_COUNT.store(0, std::sync::atomic::Ordering::SeqCst);
}

/// Reads the current value of the internal glob-compile call counter.
pub fn glob_compile_count() -> usize {
    GLOB_COMPILE_COUNT.load(std::sync::atomic::Ordering::SeqCst)
}

/// Selects the highest-versioned tag matching `template` out of a full tag
/// list previously obtained via [`fetch_all_tags`], reusing a `cache` of
/// already-compiled/already-scanned candidates keyed by `template.glob()`
/// -- a `[[package-set]]` rule with a fixed (non-`{name}`) `tag-template`
/// string applies the identical template to every matching package, so
/// without this cache `TagIndex::build`'s per-package loop would recompile
/// the same `globset::Glob` and rescan the full tag list once per package
/// sharing that template, instead of once per distinct template.
fn select_from_tags_cached<'a>(
    all_tags: &'a [String],
    template: &TagTemplate,
    grammar: VersionGrammar,
    cache: &mut std::collections::HashMap<String, Vec<&'a str>>,
) -> Result<LastTagSelection, GraphError> {
    let glob = template.glob();
    let candidates = match cache.get(&glob) {
        Some(cached) => cached.clone(),
        None => {
            let matched = matching_tags(all_tags, template)?;
            cache.insert(glob, matched.clone());
            matched
        }
    };
    select_last_tag(template, grammar, candidates).map_err(GraphError::from)
}

pub struct TagIndex {
    last: BTreeMap<PackageId, Option<LastTag>>,
    templates: BTreeMap<PackageId, TagTemplate>,
    pre_cursor: BTreeMap<PackageId, Option<CommitSha>>,
    /// The full, unfiltered raw tag list fetched once during `build` (see
    /// `fetch_all_tags`), kept around so callers checking whether a specific
    /// tag name already exists (e.g. the unreleased-version check, once per
    /// package) can consult this in-memory set instead of re-querying `git`
    /// once per check.
    all_tags: std::collections::BTreeSet<String>,
    pub diagnostics: Vec<Diagnostic>,
}

impl TagIndex {
    pub fn build<D: DependencyResolver>(
        git: &GitAccess<'_>,
        graph: &D,
        cfg: &ResolvedConfig,
    ) -> Result<Self, GraphError> {
        let mut last = BTreeMap::new();
        let mut templates = BTreeMap::new();
        let mut pre_cursor = BTreeMap::new();
        let diagnostics = Vec::new();

        // Fetch the raw tag list exactly once for the whole build, not once
        // per package -- see `fetch_all_tags` for why.
        let all_tags = fetch_all_tags(git)?;
        let all_tags_set: std::collections::BTreeSet<String> = all_tags.iter().cloned().collect();
        let mut glob_cache: std::collections::HashMap<String, Vec<&str>> = std::collections::HashMap::new();

        for pkg in graph.packages() {
            // `TagTemplate::default_for` is the single source of truth for a
            // package's default tag template -- matches `release.rs`'s own
            // `unwrap_or_else(|| callisto_model::TagTemplate::default_for(&package.id))`.
            // Deliberately not `TagTemplate::parse(&format!("{name}@{{version}}"))`:
            // `parse` additionally validates git-ref-name legality, which
            // `default_for` does not, so the two could previously reject/accept a
            // package name differently. Using `default_for` here removes that
            // extra validation from this call site to align with the
            // already-shipped `release.rs` behavior.
            let tmpl = pkg
                .tag_template
                .clone()
                .unwrap_or_else(|| TagTemplate::default_for(&pkg.id));
            let grammar = pkg.version_grammar()?;
            let mut chosen = select_from_tags_cached(&all_tags, &tmpl, grammar, &mut glob_cache)?.chosen;
            if chosen.is_none() {
                if let Some(previous) = crate::config::resolve::resolve_package_config(&pkg.id, cfg)?
                    .map(|package_config| package_config.previous_tag_templates.as_slice())
                {
                    for template in previous {
                        chosen = select_from_tags_cached(&all_tags, template, grammar, &mut glob_cache)?.chosen;
                        if chosen.is_some() {
                            break;
                        }
                    }
                }
            }
            last.insert(pkg.id.clone(), chosen);
            templates.insert(pkg.id.clone(), tmpl);
            pre_cursor.insert(pkg.id.clone(), None);
        }

        Ok(TagIndex {
            last,
            templates,
            pre_cursor,
            all_tags: all_tags_set,
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

    /// Returns whether `tag_name` is present in the raw tag list fetched
    /// during `build` -- an exact match, not a glob. Lets callers checking
    /// tag existence once per package (e.g. `derive_unreleased_decision`)
    /// consult this in-memory set instead of shelling out to `git` again
    /// for each one.
    pub fn contains_tag(&self, tag_name: &str) -> bool {
        self.all_tags.contains(tag_name)
    }

    /// Constructs an empty `TagIndex` with no packages or tags — useful in
    /// unit tests that do not exercise tag-based logic.
    pub fn empty() -> Self {
        TagIndex {
            last: BTreeMap::new(),
            templates: BTreeMap::new(),
            pre_cursor: BTreeMap::new(),
            all_tags: std::collections::BTreeSet::new(),
            diagnostics: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use callisto_model::{
        CommandError, CommandOutput, CommandRunner, DepEdge, ManifestDecl, ManifestFormat, ManifestRole, Package,
    };

    fn make_pkg(name: &str) -> Package {
        let manifest = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
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
    /// invocation, to count how many `git` spawns a `TagIndex::build` call
    /// costs.
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

    /// A directory outside any Git repository, so every `git` call is
    /// answered by the test's runner.
    fn non_repo_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    /// Spec: `TagIndex::build` selects each package's last tag from the
    /// `git tag --list` output.
    #[test]
    fn test_tag_index_build_selects_last_tag_from_git_tag_list() {
        let dir = non_repo_dir();
        let runner = FakeGitTagRunner::new(vec!["pkg-a@2.0.0".to_string()]);
        let graph = FixedGraph {
            pkgs: vec![make_pkg("pkg-a")],
        };
        let cfg = crate::config::load(dir.path()).unwrap();
        let git = GitAccess::new(dir.path(), &runner);

        let tags = TagIndex::build(&git, &graph, &cfg).expect("TagIndex::build must succeed from the git tag list");

        let pkg_id = PackageId::parse("pkg-a").unwrap();
        assert_eq!(
            tags.last_tag(&pkg_id).map(|t| t.version.render().to_string()),
            Some("2.0.0".to_string())
        );
    }

    /// Spec: `TagIndex::build` must fetch the raw tag list exactly once per
    /// build, not once per package -- each `CommandRunner` round-trip is a
    /// subprocess spawn, so N packages must not cost N round-trips.
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
        let git = GitAccess::new(dir.path(), &runner);

        let tags = TagIndex::build(&git, &graph, &cfg).unwrap();

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

    /// Spec: `matching_tags` selects exactly what the template's `globset`
    /// glob matches.
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
                Err(GraphError::Vcs(callisto_model::vcs::VcsError::InvalidGlob { .. }))
            ),
            "malformed glob must be surfaced as Err, not silently match every tag; got {result:?}"
        );
    }

    /// Spec: the malformed-glob error must propagate all the way up through
    /// `TagIndex::build`, not just the internal `matching_tags` helper.
    /// Previously covered via `last_tag_for`'s own test (deleted alongside
    /// `last_tag_for` as dead production code) -- that test exercised this
    /// exact `select_from_tags_cached`/`matching_tags` path from the other
    /// (now-removed) public entry point, so this replaces it against the
    /// surviving one.
    #[test]
    fn test_tag_index_build_propagates_malformed_glob_error() {
        let dir = non_repo_dir();
        let runner = FakeGitTagRunner::new(vec!["pkg-a@1.0.0".to_string()]);
        let mut pkg = make_pkg("pkg-a");
        pkg.tag_template = Some(TagTemplate::parse("pkg-a@{version}{oops").unwrap());
        let graph = FixedGraph { pkgs: vec![pkg] };
        let cfg = crate::config::load(dir.path()).unwrap();
        let git = GitAccess::new(dir.path(), &runner);

        match TagIndex::build(&git, &graph, &cfg) {
            Err(GraphError::Vcs(callisto_model::vcs::VcsError::InvalidGlob { .. })) => {}
            Err(other) => panic!("expected InvalidGlob, got a different GraphError: {other:?}"),
            Ok(_) => panic!("TagIndex::build must propagate the malformed-glob error, got Ok"),
        }
    }

    #[test]
    fn test_tag_index_build_with_zero_tags_returns_none_for_every_package() {
        let dir = non_repo_dir();
        let runner = FakeGitTagRunner::new(vec![]);
        let graph = FixedGraph {
            pkgs: vec![make_pkg("pkg-a"), make_pkg("pkg-b")],
        };
        let cfg = crate::config::load(dir.path()).unwrap();
        let git = GitAccess::new(dir.path(), &runner);

        let tags = TagIndex::build(&git, &graph, &cfg).unwrap();

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
        let git = GitAccess::new(dir.path(), &runner);

        let tags = TagIndex::build(&git, &graph, &cfg).unwrap();

        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let pkg_ab = PackageId::parse("pkg-ab").unwrap();
        assert_eq!(
            tags.last_tag(&pkg_a).map(|t| t.version.render().to_string()),
            Some("1.5.0".to_string())
        );
        assert_eq!(
            tags.last_tag(&pkg_ab).map(|t| t.version.render().to_string()),
            Some("9.1.0".to_string())
        );
    }

    /// A `CommandRunner` double that always fails, standing in for a
    /// missing `git` binary.
    struct FailingRunner;

    impl CommandRunner for FailingRunner {
        fn run(&self, _program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
            Err(CommandError::NotFound {
                program: "git".to_string(),
            })
        }
    }

    /// Spec: `TagIndex::build` must use `pkg.tag_template` when it is set
    /// instead of always defaulting to `{name}@{version}`. A package with
    /// `tag_template: Some(TagTemplate::parse("v{version}"))` must resolve
    /// tags like `"v1.2.3"`, and a package with `tag_template: None` must
    /// still fall back to the default `{name}@{version}` pattern.
    #[test]
    fn tag_index_uses_custom_tag_template_when_set() {
        let dir = non_repo_dir();
        let runner = FakeGitTagRunner::new(vec!["v1.2.3".to_string(), "pkg-default@4.5.6".to_string()]);

        let manifest = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();

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
        let git = GitAccess::new(dir.path(), &runner);

        let tags = TagIndex::build(&git, &graph, &cfg).expect("TagIndex::build must succeed");

        let custom_id = PackageId::parse("custom-pkg").unwrap();
        let default_id = PackageId::parse("pkg-default").unwrap();

        assert_eq!(
            tags.last_tag(&custom_id).map(|t| t.version.render().to_string()),
            Some("1.2.3".to_string()),
            "package with custom tag_template 'v{{version}}' must resolve 'v1.2.3'"
        );
        assert_eq!(
            tags.last_tag(&default_id).map(|t| t.version.render().to_string()),
            Some("4.5.6".to_string()),
            "package with no tag_template must fall back to 'pkg-default@{{version}}'"
        );
    }

    /// Spec: for a package with no explicit `tag_template`, `TagIndex::build`
    /// must resolve the same default template `TagTemplate::default_for`
    /// would produce -- `release.rs`'s existing tag-name construction already
    /// calls `default_for` directly via
    /// `unwrap_or_else(|| callisto_model::TagTemplate::default_for(&package.id))`,
    /// and `TagIndex::build`'s own default must agree with it byte-for-byte
    /// rather than re-deriving the same value through a separate
    /// `TagTemplate::parse(&format!(...))` call that could diverge (e.g. by
    /// additionally rejecting a package name `default_for` would silently
    /// accept).
    #[test]
    fn tag_index_default_template_matches_tag_template_default_for() {
        let dir = non_repo_dir();
        let runner = FakeGitTagRunner::new(vec![]);
        let graph = FixedGraph {
            pkgs: vec![make_pkg("pkg-a")],
        };
        let cfg = crate::config::load(dir.path()).unwrap();
        let git = GitAccess::new(dir.path(), &runner);

        let tags = TagIndex::build(&git, &graph, &cfg).expect("TagIndex::build must succeed");

        let pkg_id = PackageId::parse("pkg-a").unwrap();
        assert_eq!(
            tags.template(&pkg_id),
            &TagTemplate::default_for(&pkg_id),
            "TagIndex::build's default template must match TagTemplate::default_for exactly"
        );
    }

    // A `glob_compile_count()`-based regression test for this behavior lives
    // in `tests/tag_glob_cache_count_test.rs`, not here: `GLOB_COMPILE_COUNT`
    // is a process-global counter, and this module's `--lib` test binary
    // runs many other, non-`#[serial]` `TagIndex::build`/`matching_tags`
    // tests concurrently that would race an exact-count assertion (same
    // hazard `OPEN_CALL_COUNT`/`PERSIST_CALL_COUNT` are isolated from --
    // see `tests/apply_persist_open_count_test.rs`).

    /// Spec: when the `CommandRunner` returns `Err`, `TagIndex::build` must
    /// propagate that error up
    /// through the whole build rather than panicking or silently swallowing
    /// it into an empty tag list per-package.
    #[test]
    fn test_tag_index_build_propagates_command_runner_error() {
        let dir = non_repo_dir();
        let runner = FailingRunner;
        let graph = FixedGraph {
            pkgs: vec![make_pkg("pkg-a")],
        };
        let cfg = crate::config::load(dir.path()).unwrap();
        let git = GitAccess::new(dir.path(), &runner);

        let is_command_err = matches!(
            TagIndex::build(&git, &graph, &cfg),
            Err(GraphError::Vcs(callisto_model::vcs::VcsError::Command(_)))
        );

        assert!(
            is_command_err,
            "TagIndex::build must propagate the CommandRunner error as GraphError::Command"
        );
    }
}
