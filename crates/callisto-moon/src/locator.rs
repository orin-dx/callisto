use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use callisto_graph::identity::IdentityResolver;
use callisto_graph::locate::{LocateError, ProjectLocator};
use callisto_model::{
    CommandRunner, DeclaredEdge, DeclaredEdgeKind, Ecosystem, PackageId, ProjectRoot,
};
use serde::{Deserialize, Serialize};

pub struct MoonProjectLocator<'a, R: CommandRunner> {
    runner: &'a R,
    workspace_root: PathBuf,
    identity: IdentityResolver,
    graph: OnceLock<MoonProjectGraph>,
}

impl<'a, R: CommandRunner> MoonProjectLocator<'a, R> {
    pub fn new(runner: &'a R, workspace_root: PathBuf) -> Result<Self, LocateError> {
        let identity =
            IdentityResolver::new(&workspace_root).map_err(|e| LocateError::Graph(Box::new(e)))?;
        Ok(Self {
            runner,
            workspace_root,
            identity,
            graph: OnceLock::new(),
        })
    }

    fn load_graph(&self) -> Result<&MoonProjectGraph, LocateError> {
        if let Some(g) = self.graph.get() {
            return Ok(g);
        }

        let out = self
            .runner
            .run("moon", &["project-graph", "--json"], &self.workspace_root)
            .map_err(|_| LocateError::MoonUnavailable)?;

        if out.exit_code != Some(0) {
            return Err(LocateError::MoonUnavailable);
        }

        let graph: MoonProjectGraph =
            serde_json::from_str(&out.stdout).map_err(|e| LocateError::MoonOutputParse {
                message: e.to_string(),
            })?;

        let _ = self.graph.set(graph);
        Ok(self.graph.get().unwrap())
    }

    fn resolve_id(&self, root: &Path, eco: Ecosystem) -> Result<PackageId, LocateError> {
        self.identity
            .resolve(root, eco)
            .map_err(|e| LocateError::Graph(Box::new(e)))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MoonProjectGraph {
    pub projects: Vec<MoonProject>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MoonProject {
    pub root: PathBuf,
    #[serde(default)]
    pub depends_on: Vec<MoonDependency>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MoonDependency {
    pub project_root: PathBuf,
    pub scope: DependencyScope,
    #[serde(default)]
    pub via: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DependencyScope {
    Build,
    Development,
    Peer,
    Production,
    Root,
}

pub fn scope_to_declared_edge_kind(scope: DependencyScope) -> DeclaredEdgeKind {
    match scope {
        DependencyScope::Build => DeclaredEdgeKind::Build,
        DependencyScope::Development => DeclaredEdgeKind::Development,
        DependencyScope::Peer => DeclaredEdgeKind::Peer,
        DependencyScope::Production => DeclaredEdgeKind::Production,
        DependencyScope::Root => DeclaredEdgeKind::Root,
    }
}

impl<'a, R: CommandRunner> ProjectLocator for MoonProjectLocator<'a, R> {
    fn projects(&self) -> Result<Vec<ProjectRoot>, LocateError> {
        let graph = self.load_graph()?;
        let mut roots = Vec::new();

        for project in &graph.projects {
            let abs_path = if project.root.is_absolute() {
                project.root.clone()
            } else {
                self.workspace_root.join(&project.root)
            };

            let has_cargo = abs_path.join("Cargo.toml").exists();
            let has_npm = abs_path.join("package.json").exists();

            if has_cargo {
                let id = self.resolve_id(&abs_path, Ecosystem::Cargo)?;
                roots.push(ProjectRoot {
                    id,
                    path: project.root.clone(),
                    ecosystem: Ecosystem::Cargo,
                });
            }
            if has_npm {
                let id = self.resolve_id(&abs_path, Ecosystem::Npm)?;
                roots.push(ProjectRoot {
                    id,
                    path: project.root.clone(),
                    ecosystem: Ecosystem::Npm,
                });
            }
        }

        Ok(roots)
    }

    fn declared_edges(&self) -> Option<Vec<DeclaredEdge>> {
        let graph = self.load_graph().ok()?;
        let mut edges = Vec::new();

        for project in &graph.projects {
            let abs_from = if project.root.is_absolute() {
                project.root.clone()
            } else {
                self.workspace_root.join(&project.root)
            };

            for dep in &project.depends_on {
                let abs_to = if dep.project_root.is_absolute() {
                    dep.project_root.clone()
                } else {
                    self.workspace_root.join(&dep.project_root)
                };

                let from_eco = if abs_from.join("Cargo.toml").exists() {
                    Ecosystem::Cargo
                } else if abs_from.join("package.json").exists() {
                    Ecosystem::Npm
                } else {
                    continue;
                };

                let to_eco = if abs_to.join("Cargo.toml").exists() {
                    Ecosystem::Cargo
                } else if abs_to.join("package.json").exists() {
                    Ecosystem::Npm
                } else {
                    continue;
                };

                if let (Ok(from), Ok(to)) = (
                    self.resolve_id(&abs_from, from_eco),
                    self.resolve_id(&abs_to, to_eco),
                ) {
                    edges.push(DeclaredEdge {
                        from,
                        to,
                        kind: scope_to_declared_edge_kind(dep.scope),
                        via: dep.via.clone(),
                    });
                }
            }
        }

        Some(edges)
    }
}
