//! Dependency graph, cascade, aggregation, config resolution, version planning and release derivation and execution
//! for callisto.

#![allow(clippy::result_large_err)]

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use callisto_manifests::Manifest;
use callisto_model::vcs::GitAccess;
use callisto_model::{CommandRunner, PackageId, Version};

mod aggregate;
pub mod apply;
pub mod cascade;
pub mod changed;
pub mod changelog;
pub mod commands;
pub mod config;
pub mod conventional;
pub mod error;
pub mod groups;
mod history;
pub mod identity;
pub mod infer;
pub mod locate;
mod manifest_cache;
pub(crate) mod matrix;
pub mod napi;
mod parallel;
pub mod plan;
pub(crate) mod registry_endpoint;
pub mod resolver;
pub mod tags;
pub mod toposort;
pub mod walk;
mod workspace_owner;

pub use aggregate::{
    aggregate, load_changesets, load_changesets_permissive, Aggregation, ChangesetLoadFailure, LoadedChangeset, NamedBy,
};
pub use apply::{apply_version_plan, ApplyOptions, ApplyOutcome};
pub use cascade::{
    cascade_action, coverage, rewrite_spec, run_cascade, CascadeDecision, CascadeInput, CascadeOutcome, DepWriteTarget,
    RewriteKey, RewriteOutcome, SpecRewrite,
};
pub use config::{load as load_config, resolve as resolve_config, GroupDef, GroupTable, ResolvedConfig};
pub use error::{ConfigError, GraphError};
pub use groups::{fixed_group_target, pre_mutation_checks, versioned_bump, GroupCheckOutcome};
pub use identity::IdentityIndex;
pub use infer::{InferenceOutcome, InferenceWindowSpec, NoInference, SeverityInference};
pub use locate::{find_workspace_root, IgnoreWalkLocator, LocateError, ProjectLocator};
pub use napi::{napi_drift, role_to_triple, triple_to_role, NapiTargetsIndex};
pub use plan::{PlannedBump, VersionPlan, VersionWriteTarget};
pub use resolver::{DependencyResolver, ManifestWalkResolver};
pub use tags::TagIndex;
pub use toposort::toposort_impl;

pub struct Workspace<'a, R: CommandRunner, D: DependencyResolver = ManifestWalkResolver> {
    pub root: PathBuf,
    pub config: ResolvedConfig,
    pub graph: D,
    /// Deferred [`TagIndex`]: built at most once, the first time
    /// [`Workspace::tags`] is called, not eagerly by [`Workspace::load`].
    ///
    /// `TagIndex::build` fetches the repo's full tag list with a `git tag
    /// --list` spawn. Several command paths never consult tags at all (`add`'s
    /// non-interactive path only needs [`Workspace::root`]; `init` only
    /// needs package names/root), so building unconditionally in
    /// `Workspace::load` charged every caller for work only some need. All
    /// of `TagIndex::build`'s inputs are already `Workspace` fields, so a
    /// `OnceCell` needs no extra state -- go through [`Workspace::tags`],
    /// not this field directly.
    pub tags: OnceCell<TagIndex>,
    /// Deferred [`GitAccess`], built the first time [`Workspace::git_access`]
    /// is called. `pub` so tests can hand-construct a `Workspace` with a
    /// pre-seeded value.
    pub git: OnceCell<GitAccess<'a>>,
    pub runner: &'a R,
    /// Path-keyed cache of manifest handles opened read-only during this
    /// workspace's lifetime. Populated during graph discovery
    /// (`ManifestWalkResolver::build`) and reused by read-only accessors
    /// such as [`Workspace::base_versions`] so a given manifest is opened
    /// (read + parsed) at most once per command run. Never consulted by the
    /// manifest-open-for-write path in `apply.rs`, which always needs a
    /// fresh, exclusively-owned `&mut` handle.
    pub manifest_cache: RefCell<BTreeMap<PathBuf, Arc<dyn Manifest>>>,
    pub identity: IdentityIndex,
}

impl<'a, R: CommandRunner> Workspace<'a, R, ManifestWalkResolver> {
    /// Loads a workspace that is proven complete: every discovered manifest was opened and its publish targets and
    /// dependency edges resolved. Every command that plans or applies a release loads through here.
    ///
    /// # Errors
    ///
    /// Discovery or config loading fails, or a discovered manifest could not be resolved
    /// ([`GraphError::IncompleteWorkspace`]).
    pub fn load<L: ProjectLocator>(root: PathBuf, locator: &L, runner: &'a R) -> Result<Self, GraphError> {
        let config = config::load(&root)?;
        Self::load_with_config(root, config, locator, runner)
    }

    /// Like [`Workspace::load`] against an already-resolved config instead of `callisto.toml`.
    pub fn load_with_config<L: ProjectLocator>(
        root: PathBuf,
        config: ResolvedConfig,
        locator: &L,
        runner: &'a R,
    ) -> Result<Self, GraphError> {
        let workspace = Self::assemble(root, config, locator, runner)?;
        workspace.require_complete()?;
        Ok(workspace)
    }

    fn assemble<L: ProjectLocator>(
        root: PathBuf,
        mut config: ResolvedConfig,
        locator: &L,
        runner: &'a R,
    ) -> Result<Self, GraphError> {
        let manifest_cache: RefCell<BTreeMap<PathBuf, Arc<dyn Manifest>>> = RefCell::new(BTreeMap::new());
        let graph = ManifestWalkResolver::build(&root, locator, runner, &config, &manifest_cache)?;

        config.groups = GroupTable::resolve(&config.raw_groups, graph.identity())?;

        let identity = graph.identity().clone();

        Ok(Workspace {
            root,
            config,
            graph,
            tags: OnceCell::new(),
            git: OnceCell::new(),
            runner,
            manifest_cache,
            identity,
        })
    }
}

impl<'a, R: CommandRunner, D: DependencyResolver> Workspace<'a, R, D> {
    /// Fails if the walk dropped a manifest it could not resolve, because a release planned from the rest would be
    /// smaller than the repository.
    ///
    /// # Errors
    ///
    /// [`GraphError::IncompleteWorkspace`] naming every unresolved manifest.
    pub fn require_complete(&self) -> Result<(), GraphError> {
        // One manifest can fail in several stages of the walk; name it once.
        let mut seen = BTreeSet::new();
        let unresolved: Vec<String> = self
            .graph
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == callisto_model::DiagnosticCode::ManifestUnresolved)
            .filter(|diagnostic| diagnostic.path.as_ref().is_none_or(|path| seen.insert(path.clone())))
            .map(|diagnostic| diagnostic.message.clone())
            .collect();
        if unresolved.is_empty() {
            Ok(())
        } else {
            Err(GraphError::IncompleteWorkspace { unresolved })
        }
    }

    /// Each package's id as reports print it.
    pub fn report_ids(&self) -> plan::ReportIds {
        plan::ReportIds::new(self.graph.packages())
    }

    /// Returns the workspace's [`TagIndex`], building it on first access and
    /// reusing the cached result afterwards. See the doc comment on the
    /// `tags` field for why this is deferred rather than built eagerly by
    /// [`Workspace::load`].
    pub fn tags(&self) -> Result<&TagIndex, GraphError> {
        if let Some(existing) = self.tags.get() {
            return Ok(existing);
        }
        let built = TagIndex::build(self.git_access(), &self.graph, &self.identity, &self.config)?;
        // `OnceCell::set` only fails if another write already raced it in;
        // `Workspace` is only ever accessed through `&self` here (never
        // shared across threads -- `R`/`D` carry no such bound), so the
        // `get()` check above already ruled that out. Fall back to `get()`
        // either way rather than trusting the `set` call's own return value,
        // so this stays correct even if that assumption ever changes.
        self.tags.set(built).ok();
        Ok(self
            .tags
            .get()
            .expect("tags was just set above, or already set by a prior call"))
    }

    /// Returns the workspace's shared [`GitAccess`], rooted at the workspace
    /// root and running `git` through the workspace's runner.
    pub fn git_access(&self) -> &GitAccess<'a> {
        self.git.get_or_init(|| GitAccess::new(&self.root, self.runner))
    }

    pub fn base_versions(&self) -> Result<BTreeMap<PackageId, Version>, GraphError> {
        let ctx = callisto_manifests::OpenContext::for_workspace_root(&self.root)?;

        let mut versions = BTreeMap::new();
        for pkg in self.graph.packages() {
            let mut found_version = None;
            for decl in &pkg.manifests {
                if decl.role == callisto_model::ManifestRole::Canonical {
                    let handle = manifest_cache::open_cached(&self.manifest_cache, decl, &ctx)?;
                    let v = handle.current_version()?;
                    found_version = Some(v);
                    break;
                }
            }
            if let Some(version) = found_version {
                versions.insert(pkg.id.clone(), version);
            } else {
                return Err(GraphError::Manifest(callisto_model::ManifestError::MissingField {
                    path: pkg.manifests.first().map(|m| m.path.clone()).unwrap_or_default(),
                    field: "version",
                }));
            }
        }
        Ok(versions)
    }

    pub fn initial_versions(&self) -> Result<Vec<(String, Version)>, GraphError> {
        let base = self.base_versions()?;
        Ok(base.into_iter().map(|(id, v)| (pre_json_key(&id), v)).collect())
    }

    /// Resolves stored baselines to their owners before applying the owners' native version grammars.
    pub fn resolve_pre_state(
        &self,
        state: callisto_model::format::PreState,
    ) -> Result<callisto_model::format::PreState, GraphError> {
        let raw_versions = state
            .initial_versions
            .iter()
            .map(|(key, version)| (key.clone(), version.raw().to_string()))
            .collect();
        self.resolve_pre_state_with_raw_versions(state, &raw_versions)
    }

    /// Resolves stored baselines using their original JSON spellings.
    pub fn resolve_pre_state_from_json(
        &self,
        state: callisto_model::format::PreState,
        original_json: &str,
    ) -> Result<callisto_model::format::PreState, GraphError> {
        let clean_json = original_json.strip_prefix('\u{FEFF}').unwrap_or(original_json);
        let value: serde_json::Value = serde_json::from_str(clean_json).map_err(|error| {
            GraphError::PreJson(callisto_model::format::PreJsonError::Malformed {
                message: error.to_string(),
            })
        })?;
        let raw_versions: indexmap::IndexMap<String, String> = value
            .get("initialVersions")
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flat_map(|values| values.iter())
            .filter_map(|(key, value)| value.as_str().map(|raw| (key.clone(), raw.to_string())))
            .collect();
        self.resolve_pre_state_with_raw_versions(state, &raw_versions)
    }

    fn resolve_pre_state_with_raw_versions(
        &self,
        state: callisto_model::format::PreState,
        raw_versions: &indexmap::IndexMap<String, String>,
    ) -> Result<callisto_model::format::PreState, GraphError> {
        assign_promoted_pre_keys(
            self.git_access(),
            &self.graph,
            &self.identity,
            &self.config.pre_json_path(),
            state,
            raw_versions,
        )
    }
}

/// The `pre.json` `initialVersions` key for `id`: its display id, so `cargo/foo` and `npm/foo` never share an entry.
pub fn pre_json_key(id: &PackageId) -> String {
    id.display_name()
}

/// `id`'s pinned pre-release baseline.
pub fn pre_initial_version<'a>(pre: &'a callisto_model::format::PreState, id: &PackageId) -> Option<&'a Version> {
    pre.initial_versions.get(&pre_json_key(id))
}

/// The commit that added `pre_json`, asked of Git once however many keys need it.
fn commit_added<'c>(
    cache: &'c mut Option<Option<String>>,
    git: &GitAccess<'_>,
    pre_json: &std::path::Path,
) -> Result<Option<&'c str>, GraphError> {
    if cache.is_none() {
        *cache = Some(git.commit_adding(pre_json).map_err(GraphError::Vcs)?);
    }
    Ok(cache.as_ref().and_then(Option::as_deref))
}

/// Whether `package`, which carries the bare `key` today, may be the package that held it when `pre.json` was added.
///
/// History that cannot be read is no evidence of another owner, so it keeps the key.
fn bare_key_may_belong(
    git: &GitAccess<'_>,
    added: &mut Option<Option<String>>,
    pre_json: &std::path::Path,
    key: &str,
    package: &callisto_model::Package,
    identity: &IdentityIndex,
    rosters: &mut history::Rosters,
) -> bool {
    let held = commit_added(added, git, pre_json).and_then(|rev| match rev {
        Some(rev) => history::may_have_held_name_at(git, rev, key, package, identity, rosters),
        None => Ok(true),
    });
    held.unwrap_or(true)
}

/// Resolves native baselines, attributing historical bare keys at the commit that added `pre.json`.
pub(crate) fn assign_promoted_pre_keys<D: DependencyResolver>(
    git: &GitAccess<'_>,
    graph: &D,
    identity: &IdentityIndex,
    pre_json: &std::path::Path,
    mut state: callisto_model::format::PreState,
    raw_versions: &indexmap::IndexMap<String, String>,
) -> Result<callisto_model::format::PreState, GraphError> {
    let keys: Vec<String> = state.initial_versions.keys().cloned().collect();
    let mut owners = BTreeMap::new();
    let mut rosters = history::Rosters::default();
    let mut added = None;
    for key in &keys {
        if let Some(owner) = graph.packages().find(|p| pre_json_key(&p.id) == *key) {
            if owner.id.ecosystem().is_some()
                || bare_key_may_belong(git, &mut added, pre_json, key, owner, identity, &mut rosters)
            {
                owners.insert(key.clone(), owner);
            } else {
                // Another package's baseline: it must not pin this package's version either.
                state.initial_versions.shift_remove(key);
            }
            continue;
        }
        let same: Vec<&callisto_model::Package> = graph
            .packages()
            .filter(|p| p.id.ecosystem().is_some() && p.id.name() == key.as_str())
            .collect();
        let owner = if same.is_empty() {
            match identity.resolve(key) {
                Ok(id) => {
                    let package = graph.packages().find(|p| p.id == id);
                    package.filter(|package| {
                        PackageId::parse(key).is_ok_and(|parsed| parsed.ecosystem().is_some())
                            || bare_key_may_belong(git, &mut added, pre_json, key, package, identity, &mut rosters)
                    })
                }
                Err(GraphError::UnknownPackage { .. }) => None,
                Err(GraphError::AmbiguousName { candidates, .. }) => {
                    return Err(GraphError::AmbiguousPreJsonKey {
                        key: key.clone(),
                        candidates,
                    });
                }
                Err(error) => return Err(error),
            }
        } else {
            let owner = match commit_added(&mut added, git, pre_json)? {
                Some(rev) => history::sole_owner_at(git, rev, key, &same, identity, &mut rosters)?,
                None => None,
            };
            Some(owner.ok_or_else(|| GraphError::AmbiguousPreJsonKey {
                key: key.clone(),
                candidates: same.iter().map(|p| p.id.clone()).collect(),
            })?)
        };
        if let Some(owner) = owner {
            owners.insert(key.clone(), owner);
        }
    }
    state
        .resolve_initial_versions_from_raw(raw_versions, |key| {
            owners
                .get(key)
                .and_then(|owner| owner.canonical_manifests().next().map(|manifest| manifest.ecosystem()))
        })
        .map_err(GraphError::PreJson)?;
    let mut legacy_by_owner_key: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for key in keys {
        if let Some(owner) = owners.get(&key) {
            let owner_key = pre_json_key(&owner.id);
            if owner_key != key {
                legacy_by_owner_key.entry(owner_key).or_default().push(key);
            }
        }
    }
    // Every key for one package must agree before anything moves.
    for (owner_key, legacy_keys) in &legacy_by_owner_key {
        let reference = if state.initial_versions.contains_key(owner_key) {
            owner_key
        } else {
            &legacy_keys[0]
        };
        let reference_version = &state.initial_versions[reference];
        for key in legacy_keys.iter().filter(|key| *key != reference) {
            let version = &state.initial_versions[key];
            if version.compare(reference_version) != Ok(std::cmp::Ordering::Equal) {
                return Err(GraphError::ConflictingPreJsonBaselines {
                    key: key.clone(),
                    version: version.clone(),
                    other_key: reference.clone(),
                    other_version: reference_version.clone(),
                });
            }
        }
    }
    for (owner_key, legacy_keys) in legacy_by_owner_key {
        for key in legacy_keys {
            if let Some(version) = state.initial_versions.shift_remove(&key) {
                state.initial_versions.entry(owner_key.clone()).or_insert(version);
            }
        }
    }
    Ok(state)
}

#[cfg(test)]
mod pre_json_key_tests {
    use super::*;
    use callisto_model::format::PreState;
    use callisto_model::Ecosystem;

    fn prefixed(ecosystem: Ecosystem, name: &str) -> PackageId {
        PackageId::Prefixed {
            ecosystem,
            name: name.to_string(),
        }
    }

    #[test]
    fn same_named_packages_in_two_ecosystems_keep_separate_baselines() {
        let cargo = prefixed(Ecosystem::Cargo, "foo");
        let npm = prefixed(Ecosystem::Npm, "foo");
        let pre = PreState::entering(
            "beta",
            [
                (pre_json_key(&cargo), Version::semver(1, 0, 0)),
                (pre_json_key(&npm), Version::semver(2, 0, 0)),
            ],
        );
        assert_eq!(pre_initial_version(&pre, &cargo), Some(&Version::semver(1, 0, 0)));
        assert_eq!(pre_initial_version(&pre, &npm), Some(&Version::semver(2, 0, 0)));
    }
}
