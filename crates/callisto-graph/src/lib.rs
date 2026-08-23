//! Dependency graph, cascade, aggregation, and config resolution for callisto.

#![allow(clippy::result_large_err)]

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use callisto_manifests::Manifest;
use callisto_model::{CommandRunner, Ecosystem, PackageId, Version};
use callisto_vcs::GitAccess;

pub mod aggregate;
pub mod apply;
pub mod cascade;
pub mod changed;
pub mod commands;
pub mod config;
pub mod crosscheck;
pub mod error;
pub mod groups;
pub mod identity;
pub mod infer;
pub mod locate;
mod manifest_cache;
pub(crate) mod matrix;
pub mod napi;
pub mod plan;
pub mod resolver;
pub mod tags;
pub mod toposort;
pub mod walk;

pub use aggregate::{aggregate, load_changesets, Aggregation, LoadedChangeset, NamedBy};
pub use apply::{apply_version_plan, ApplyOptions, ApplyOutcome};
pub use cascade::{
    cascade_action, coverage, rewrite_spec, run_cascade, CascadeDecision, CascadeInput, CascadeOutcome, DepWriteTarget,
    RewriteKey, RewriteOutcome, SpecRewrite,
};
pub use config::{load as load_config, GroupDef, GroupTable, ResolvedConfig};
pub use error::{ConfigError, GraphError};
pub use groups::{fixed_group_target, pre_mutation_checks, GroupCheckOutcome};
pub use identity::{IdentityIndex, IdentityResolver};
pub use infer::{InferenceOutcome, InferenceWindowSpec, NoInference, SeverityInference};
pub use locate::{find_workspace_root, IgnoreWalkLocator, LocateError, ProjectLocator};
pub use napi::{napi_drift, role_to_triple, triple_to_role, NapiTargetsIndex};
pub use plan::{PlannedBump, VersionPlan, VersionWriteTarget};
pub use resolver::{DependencyResolver, ManifestWalkResolver};
pub use tags::{last_tag_for, TagIndex};
pub use toposort::toposort_impl;

pub struct Workspace<'a, R: CommandRunner, D: DependencyResolver = ManifestWalkResolver> {
    pub root: PathBuf,
    pub config: ResolvedConfig,
    pub graph: D,
    /// Deferred [`TagIndex`]: built at most once, the first time
    /// [`Workspace::tags`] is called, not eagerly by [`Workspace::load`].
    ///
    /// `TagIndex::build` fetches the repo's full tag list -- native gix, or
    /// (unavailable on `wasm32`) a shelled `git tag --list` Extism
    /// round-trip. Several command paths never consult tags at all (`add`'s
    /// non-interactive path only needs [`Workspace::root`]; `init` only
    /// needs package names/root), so building unconditionally in
    /// `Workspace::load` charged every caller for work only some need. All
    /// of `TagIndex::build`'s inputs are already `Workspace` fields, so a
    /// `OnceCell` needs no extra state -- go through [`Workspace::tags`],
    /// not this field directly.
    pub tags: OnceCell<TagIndex>,
    /// Deferred [`GitAccess`]: built at most once, the first time
    /// [`Workspace::git_access`] is called, mirroring `tags` above.
    /// `GitAccess::discover` never fails (native gix, falling back to a
    /// `CommandRunner` shell round-trip when unavailable), so simpler
    /// than `tags` -- no `Result` to thread through. Consolidates what
    /// were multiple independent `GitAccess::discover` calls within one
    /// command invocation (`plan_publish`'s head_sha resolution,
    /// `TagIndex::build` via [`Workspace::tags`]) into one shared
    /// discovery. `pub` so tests can hand-construct a `Workspace` with a
    /// pre-seeded value, bypassing discovery.
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
    pub fn load<L: ProjectLocator>(root: PathBuf, locator: &L, runner: &'a R) -> Result<Self, GraphError> {
        let mut config = config::load(&root)?;
        let manifest_cache: RefCell<BTreeMap<PathBuf, Arc<dyn Manifest>>> = RefCell::new(BTreeMap::new());
        let graph = ManifestWalkResolver::build(&root, locator, runner, &config, &manifest_cache)?;

        config.groups = GroupTable::resolve(&config.raw_groups, graph.identity())?;

        {
            let mut by_name: BTreeMap<String, Vec<(PackageId, BTreeSet<Ecosystem>)>> = BTreeMap::new();
            let mut ecosystems_by_id: BTreeMap<(String, PackageId), BTreeSet<Ecosystem>> = BTreeMap::new();
            for ((eco, name), id) in &graph.identity().prefixed {
                ecosystems_by_id
                    .entry((name.clone(), id.clone()))
                    .or_default()
                    .insert(*eco);
            }
            for ((name, id), ecos) in ecosystems_by_id {
                by_name.entry(name).or_default().push((id, ecos));
            }
            config.promoted_siblings = by_name.into_iter().filter(|(_, ids)| ids.len() >= 2).collect();
        }

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
    /// Returns the workspace's [`TagIndex`], building it on first access and
    /// reusing the cached result afterwards. See the doc comment on the
    /// `tags` field for why this is deferred rather than built eagerly by
    /// [`Workspace::load`].
    pub fn tags(&self) -> Result<&TagIndex, GraphError> {
        if let Some(existing) = self.tags.get() {
            return Ok(existing);
        }
        let built = TagIndex::build(self.git_access(), &self.graph, &self.config)?;
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

    /// Returns the workspace's shared [`GitAccess`], discovering it on
    /// first access and reusing the cached result afterwards -- mirrors
    /// [`Workspace::tags`]. Every command that needs git (tag resolution,
    /// head SHA lookup, commit history walks, ...) should go through this
    /// rather than calling `GitAccess::discover` itself, so a single
    /// command invocation never pays for more than one discovery
    /// (native gix repository-open, or a `CommandRunner` shell round-trip
    /// when gix is unavailable) regardless of how many of those it needs.
    pub fn git_access(&self) -> &GitAccess<'a> {
        self.git.get_or_init(|| GitAccess::discover(&self.root, self.runner))
    }

    pub fn base_versions(&self) -> Result<BTreeMap<PackageId, Version>, GraphError> {
        let cargo_workspace = if self.root.join("Cargo.toml").exists() {
            if let Ok(resolver) = callisto_manifests::WorkspaceCargoResolver::load(&self.root.join("Cargo.toml")) {
                resolver.inheritance().ok().map(std::sync::Arc::new)
            } else {
                None
            }
        } else {
            None
        };
        let npm_workspace_kind = callisto_manifests::detect_npm_workspace_kind(&self.root).ok().flatten();
        let ctx = callisto_manifests::OpenContext {
            workspace_root: &self.root,
            cargo_workspace,
            npm_workspace_kind,
        };

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

    pub fn pre_json_key<'b>(&self, id: &'b PackageId) -> Result<&'b str, GraphError> {
        Ok(id.name())
    }

    pub fn initial_versions(&self) -> Result<Vec<(String, Version)>, GraphError> {
        let base = self.base_versions()?;
        Ok(base.into_iter().map(|(id, v)| (id.name().to_string(), v)).collect())
    }
}
