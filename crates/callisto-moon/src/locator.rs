use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use callisto_graph::identity::IdentityResolver;
use callisto_graph::locate::{LocateError, ProjectLocator};
use callisto_model::{CommandRunner, DeclaredEdge, DeclaredEdgeKind, Ecosystem, PackageId, ProjectRoot};
use serde::{Deserialize, Serialize};

pub struct MoonProjectLocator<'a, R: CommandRunner> {
    runner: &'a R,
    workspace_root: PathBuf,
    identity: IdentityResolver,
    graph: OnceLock<MoonProjectGraph>,
}

impl<'a, R: CommandRunner> MoonProjectLocator<'a, R> {
    pub fn new(runner: &'a R, workspace_root: PathBuf) -> Result<Self, LocateError> {
        let identity = IdentityResolver::new(&workspace_root).map_err(|e| LocateError::Graph(Box::new(e)))?;
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
            .map_err(|_err| LocateError::MoonUnavailable)?;

        if out.exit_code != Some(0) {
            return Err(LocateError::MoonUnavailable);
        }

        let graph: MoonProjectGraph =
            serde_json::from_str(&out.stdout).map_err(|e| LocateError::MoonOutputParse { message: e.to_string() })?;

        let _res = self.graph.set(graph);
        Ok(self.graph.get().unwrap())
    }

    fn resolve_id(&self, root: &Path, eco: Ecosystem) -> Result<PackageId, LocateError> {
        self.identity
            .resolve(root, eco)
            .map_err(|e| LocateError::Graph(Box::new(e)))
    }
}

/// Matches the real `moon project-graph --json` output shape.
/// Unknown top-level fields (e.g. `graph`) are silently ignored.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MoonProjectGraph {
    pub data: HashMap<String, MoonProject>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MoonProject {
    pub id: String,
    /// Absolute path to the project root on disk.
    pub root: PathBuf,
    /// Workspace-relative path (moon's "source" field).
    pub source: String,
    #[serde(default)]
    pub dependencies: Vec<MoonDependency>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MoonDependency {
    /// Moon project ID of the dependency (key in `data`).
    pub id: String,
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

        for project in graph.data.values() {
            // Use `source` (workspace-relative) rather than `root` (host-absolute).
            // In the WASM PDK context moon's `root` carries host-absolute paths
            // that are not in the same virtual-filesystem namespace as the
            // `/workspace/`-anchored `workspace_root` the moon host passes to
            // the extension. Using `source` works correctly in both WASM and
            // native environments.
            let rel_path = PathBuf::from(&project.source);
            let abs_path = self.workspace_root.join(&rel_path);

            if !abs_path.starts_with(&self.workspace_root) {
                return Err(LocateError::OutsideWorkspaceRoot {
                    path: abs_path,
                    root: self.workspace_root.clone(),
                });
            }

            let has_cargo = abs_path.join("Cargo.toml").exists();
            let has_npm = abs_path.join("package.json").exists();
            let has_pypi = abs_path.join("pyproject.toml").exists();

            if has_cargo {
                let id = self.resolve_id(&abs_path, Ecosystem::Cargo)?;
                roots.push(ProjectRoot {
                    id,
                    path: rel_path.clone(),
                    ecosystem: Ecosystem::Cargo,
                });
            }
            if has_npm {
                let id = self.resolve_id(&abs_path, Ecosystem::Npm)?;
                roots.push(ProjectRoot {
                    id,
                    path: rel_path.clone(),
                    ecosystem: Ecosystem::Npm,
                });
            }
            if has_pypi {
                let id = self.resolve_id(&abs_path, Ecosystem::Pypi)?;
                roots.push(ProjectRoot {
                    id,
                    path: rel_path,
                    ecosystem: Ecosystem::Pypi,
                });
            }
        }

        Ok(roots)
    }

    fn declared_edges(&self) -> Option<Vec<DeclaredEdge>> {
        let graph = self.load_graph().ok()?;

        // Build a lookup from moon project ID to project for dependency resolution.
        let id_to_project: HashMap<&str, &MoonProject> = graph.data.values().map(|p| (p.id.as_str(), p)).collect();

        let mut edges = Vec::new();

        for project in graph.data.values() {
            let abs_from = self.workspace_root.join(&project.source);

            let from_eco = if abs_from.join("Cargo.toml").exists() {
                Ecosystem::Cargo
            } else if abs_from.join("package.json").exists() {
                Ecosystem::Npm
            } else if abs_from.join("pyproject.toml").exists() {
                Ecosystem::Pypi
            } else {
                continue;
            };

            for dep in &project.dependencies {
                let Some(to_project) = id_to_project.get(dep.id.as_str()) else {
                    continue;
                };

                let abs_to = self.workspace_root.join(&to_project.source);

                let to_eco = if abs_to.join("Cargo.toml").exists() {
                    Ecosystem::Cargo
                } else if abs_to.join("package.json").exists() {
                    Ecosystem::Npm
                } else if abs_to.join("pyproject.toml").exists() {
                    Ecosystem::Pypi
                } else {
                    continue;
                };

                if let (Ok(from), Ok(to)) = (self.resolve_id(&abs_from, from_eco), self.resolve_id(&abs_to, to_eco)) {
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use callisto_model::{CommandError, CommandOutput, CommandRunner, Ecosystem};
    use tempfile::tempdir;

    use crate::MoonProjectLocator;

    struct MockMoonRunner {
        graph_json: String,
    }

    impl CommandRunner for MockMoonRunner {
        fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
            if program == "moon" && args.contains(&"project-graph") {
                Ok(CommandOutput {
                    exit_code: Some(0),
                    stdout: self.graph_json.clone(),
                    stderr: String::new(),
                })
            } else {
                Err(CommandError::NotFound {
                    program: program.to_string(),
                })
            }
        }
    }

    /// Regression test: Python projects in a moon workspace were never
    /// discovered because `projects()` only checked for `Cargo.toml` and
    /// `package.json`, not `pyproject.toml`.
    ///
    /// This test MUST FAIL before the fix (no `Ecosystem::Pypi` entry in the
    /// returned list) and pass after the fix.
    #[test]
    fn projects_discovers_pyproject_toml_as_pypi() {
        let workspace = tempdir().expect("failed to create tempdir");
        let root = workspace.path();

        // Create a Python project sub-directory with a valid pyproject.toml.
        let py_dir = root.join("py-pkg");
        std::fs::create_dir_all(&py_dir).expect("failed to create py-pkg dir");
        std::fs::write(
            py_dir.join("pyproject.toml"),
            "[project]\nname = \"my-python-package\"\nversion = \"1.0.0\"\n",
        )
        .expect("failed to write pyproject.toml");

        let graph_json = serde_json::json!({
            "data": {
                "py-pkg": {
                    "id": "py-pkg",
                    "root": py_dir.to_str().unwrap(),
                    "source": "py-pkg",
                    "dependencies": []
                }
            }
        })
        .to_string();

        let runner = MockMoonRunner { graph_json };
        let locator =
            MoonProjectLocator::new(&runner, root.to_path_buf()).expect("MoonProjectLocator::new must succeed");

        use callisto_graph::locate::ProjectLocator;
        let roots = locator.projects().expect("projects() must succeed");

        let has_pypi = roots.iter().any(|r| r.ecosystem == Ecosystem::Pypi);
        assert!(
            has_pypi,
            "expected a Pypi ProjectRoot for py-pkg/pyproject.toml, got: {roots:#?}"
        );
    }

    /// `declared_edges()` must also resolve Python projects when both the
    /// from and to projects have `pyproject.toml`.
    #[test]
    fn declared_edges_resolves_pypi_to_pypi_dependency() {
        let workspace = tempdir().expect("failed to create tempdir");
        let root = workspace.path();

        let pkg_a = root.join("pkg-a");
        let pkg_b = root.join("pkg-b");
        std::fs::create_dir_all(&pkg_a).expect("failed to create pkg-a dir");
        std::fs::create_dir_all(&pkg_b).expect("failed to create pkg-b dir");
        std::fs::write(
            pkg_a.join("pyproject.toml"),
            "[project]\nname = \"pkg-a\"\nversion = \"1.0.0\"\n",
        )
        .expect("failed to write pkg-a/pyproject.toml");
        std::fs::write(
            pkg_b.join("pyproject.toml"),
            "[project]\nname = \"pkg-b\"\nversion = \"1.0.0\"\n",
        )
        .expect("failed to write pkg-b/pyproject.toml");

        // pkg-a depends on pkg-b (production scope)
        let graph_json = serde_json::json!({
            "data": {
                "pkg-a": {
                    "id": "pkg-a",
                    "root": pkg_a.to_str().unwrap(),
                    "source": "pkg-a",
                    "dependencies": [
                        {"id": "pkg-b", "scope": "production", "via": null}
                    ]
                },
                "pkg-b": {
                    "id": "pkg-b",
                    "root": pkg_b.to_str().unwrap(),
                    "source": "pkg-b",
                    "dependencies": []
                }
            }
        })
        .to_string();

        let runner = MockMoonRunner { graph_json };
        let locator =
            MoonProjectLocator::new(&runner, root.to_path_buf()).expect("MoonProjectLocator::new must succeed");

        use callisto_graph::locate::ProjectLocator;
        let edges = locator.declared_edges().expect("declared_edges() must return Some");

        assert_eq!(
            edges.len(),
            1,
            "expected one declared edge for the pkg-a -> pkg-b dependency, got: {edges:#?}"
        );
    }

    /// Real moon `project-graph --json` emits `{"data": {...}}` (a map keyed
    /// by moon project ID) with each project's `root` as an absolute path
    /// and `source` as the workspace-relative path.  The old type used a
    /// `{"projects": [...]}` schema that was never the real moon format; this
    /// test exercises the correct schema.
    #[test]
    fn projects_uses_real_moon_data_schema() {
        let workspace = tempdir().expect("failed to create tempdir");
        let root = workspace.path();

        let py_dir = root.join("py-pkg");
        std::fs::create_dir_all(&py_dir).expect("failed to create py-pkg dir");
        std::fs::write(
            py_dir.join("pyproject.toml"),
            "[project]\nname = \"my-python-package\"\nversion = \"1.0.0\"\n",
        )
        .expect("failed to write pyproject.toml");

        // Real moon project-graph --json format: data is a HashMap keyed by
        // project ID; root is absolute; source is workspace-relative.
        let graph_json = serde_json::json!({
            "graph": {},
            "data": {
                "py-pkg": {
                    "id": "py-pkg",
                    "root": py_dir.to_str().unwrap(),
                    "source": "py-pkg",
                    "dependencies": []
                }
            }
        })
        .to_string();

        let runner = MockMoonRunner { graph_json };
        let locator =
            MoonProjectLocator::new(&runner, root.to_path_buf()).expect("MoonProjectLocator::new must succeed");

        use callisto_graph::locate::ProjectLocator;
        let roots = locator.projects().expect("projects() must succeed");

        let has_pypi = roots.iter().any(|r| r.ecosystem == Ecosystem::Pypi);
        assert!(
            has_pypi,
            "expected a Pypi ProjectRoot with real moon data schema, got: {roots:#?}"
        );
    }

    /// Existing ecosystems (Cargo, Npm) are still discovered when only checking
    /// pyproject.toml is added -- ensure no regression.
    #[test]
    fn projects_still_discovers_cargo_and_npm() {
        let workspace = tempdir().expect("failed to create tempdir");
        let root = workspace.path();

        let cargo_dir = root.join("rs-pkg");
        std::fs::create_dir_all(&cargo_dir).expect("failed to create rs-pkg dir");
        std::fs::write(
            cargo_dir.join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
        )
        .expect("failed to write Cargo.toml");

        let graph_json = serde_json::json!({
            "data": {
                "rs-pkg": {
                    "id": "rs-pkg",
                    "root": cargo_dir.to_str().unwrap(),
                    "source": "rs-pkg",
                    "dependencies": []
                }
            }
        })
        .to_string();

        let runner = MockMoonRunner { graph_json };
        let locator =
            MoonProjectLocator::new(&runner, root.to_path_buf()).expect("MoonProjectLocator::new must succeed");

        use callisto_graph::locate::ProjectLocator;
        let roots = locator.projects().expect("projects() must succeed");

        let has_cargo = roots.iter().any(|r| r.ecosystem == Ecosystem::Cargo);
        assert!(
            has_cargo,
            "expected a Cargo ProjectRoot for rs-pkg/Cargo.toml, got: {roots:#?}"
        );
    }
}
