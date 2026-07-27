//! Dependency graph, cascade, aggregation, and config resolution for callisto.

#![allow(clippy::result_large_err)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use callisto_model::{CommandRunner, PackageId, Version};

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
pub mod napi;
pub mod plan;
pub mod resolver;
pub mod tags;
pub mod toposort;
pub mod walk;

pub use aggregate::{aggregate, load_changesets, Aggregation, LoadedChangeset, NamedBy};
pub use apply::{apply_version_plan, ApplyOptions, ApplyOutcome};
pub use cascade::{
    cascade_action, coverage, rewrite_spec, run_cascade, CascadeDecision, CascadeInput,
    CascadeOutcome, DepWriteTarget, RewriteKey, RewriteOutcome, SpecRewrite,
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
    pub tags: TagIndex,
    pub runner: &'a R,
}

impl<'a, R: CommandRunner> Workspace<'a, R, ManifestWalkResolver> {
    pub fn load<L: ProjectLocator>(
        root: PathBuf,
        locator: &L,
        runner: &'a R,
    ) -> Result<Self, GraphError> {
        let config = config::load(&root)?;
        let graph = ManifestWalkResolver::build(&root, locator, runner, &config)?;
        let tags = TagIndex::build(runner, &root, &graph, &config)?;

        Ok(Workspace {
            root,
            config,
            graph,
            tags,
            runner,
        })
    }
}

impl<'a, R: CommandRunner, D: DependencyResolver> Workspace<'a, R, D> {
    pub fn base_versions(&self) -> Result<BTreeMap<PackageId, Version>, GraphError> {
        let cargo_workspace = if self.root.join("Cargo.toml").exists() {
            if let Ok(resolver) =
                callisto_manifests::WorkspaceCargoResolver::load(&self.root.join("Cargo.toml"))
            {
                resolver.inheritance().ok().map(std::sync::Arc::new)
            } else {
                None
            }
        } else {
            None
        };
        let npm_workspace_kind = callisto_manifests::detect_npm_workspace_kind(&self.root)
            .ok()
            .flatten();
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
                    let handle = callisto_manifests::open(decl, &ctx)?;
                    let v = handle.current_version()?;
                    found_version = Some(v);
                    break;
                }
            }
            if let Some(version) = found_version {
                versions.insert(pkg.id.clone(), version);
            } else {
                return Err(GraphError::Manifest(
                    callisto_model::ManifestError::MissingField {
                        path: pkg
                            .manifests
                            .first()
                            .map(|m| m.path.clone())
                            .unwrap_or_default(),
                        field: "version",
                    },
                ));
            }
        }
        Ok(versions)
    }

    pub fn pre_json_key<'b>(&self, id: &'b PackageId) -> Result<&'b str, GraphError> {
        Ok(id.name())
    }

    pub fn initial_versions(&self) -> Result<Vec<(String, Version)>, GraphError> {
        let base = self.base_versions()?;
        Ok(base
            .into_iter()
            .map(|(id, v)| (id.name().to_string(), v))
            .collect())
    }
}
