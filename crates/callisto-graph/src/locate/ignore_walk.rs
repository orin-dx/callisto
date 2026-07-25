use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use callisto_model::{workspace_relative, Ecosystem, PackageId, ProjectRoot};
use ignore::WalkBuilder;

use crate::locate::{find_workspace_root, LocateError, ProjectLocator};

pub struct IgnoreWalkLocator {
    root: PathBuf,
    skip: BTreeSet<&'static str>,
}

impl IgnoreWalkLocator {
    pub fn new(root: &Path) -> Self {
        let mut skip = BTreeSet::new();
        skip.insert("target");
        skip.insert("node_modules");
        skip.insert(".git");
        skip.insert(".moon");
        skip.insert("dist");

        let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        IgnoreWalkLocator {
            root: canonical,
            skip,
        }
    }

    pub fn discover(start: &Path) -> Result<Self, LocateError> {
        let root = find_workspace_root(start)?;
        Ok(Self::new(&root))
    }
}

impl ProjectLocator for IgnoreWalkLocator {
    fn projects(&self) -> Result<Vec<ProjectRoot>, LocateError> {
        let mut results = Vec::new();
        let walker = WalkBuilder::new(&self.root)
            .hidden(true)
            .git_ignore(true)
            .parents(false)
            .filter_entry({
                let skip = self.skip.clone();
                move |entry| {
                    if let Some(name) = entry.file_name().to_str() {
                        if skip.contains(name) {
                            return false;
                        }
                    }
                    true
                }
            })
            .build();

        for entry_res in walker {
            let entry = entry_res.map_err(|e| LocateError::Walk {
                path: self.root.clone(),
                message: e.to_string(),
            })?;

            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let cargo_toml = path.join("Cargo.toml");
            if cargo_toml.exists() {
                if let Ok(content) = fs::read_to_string(&cargo_toml) {
                    if content.contains("[package]") {
                        if let Ok(doc) = content.parse::<toml_edit::DocumentMut>() {
                            if let Some(name) = doc
                                .get("package")
                                .and_then(|p| p.get("name"))
                                .and_then(|n| n.as_str())
                            {
                                let rel = to_workspace_relative(path, &self.root)?;
                                let id = PackageId::parse(name)
                                    .unwrap_or_else(|_| PackageId::Bare(name.to_string()));
                                results.push(ProjectRoot {
                                    id,
                                    path: rel,
                                    ecosystem: Ecosystem::Cargo,
                                });
                            }
                        }
                    }
                }
            }

            let pkg_json = path.join("package.json");
            if pkg_json.exists() {
                if let Ok(content) = fs::read_to_string(&pkg_json) {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(name) = val.get("name").and_then(|n| n.as_str()) {
                            let rel = to_workspace_relative(path, &self.root)?;
                            let id = PackageId::parse(name)
                                .unwrap_or_else(|_| PackageId::Bare(name.to_string()));
                            results.push(ProjectRoot {
                                id,
                                path: rel,
                                ecosystem: Ecosystem::Npm,
                            });
                        }
                    }
                }
            }
        }

        results.sort_by(|a, b| (&a.path, a.ecosystem).cmp(&(&b.path, b.ecosystem)));
        Ok(results)
    }
}

fn to_workspace_relative(path: &Path, root: &Path) -> Result<PathBuf, LocateError> {
    if !path.starts_with(root) {
        return Err(LocateError::OutsideWorkspaceRoot {
            path: path.to_path_buf(),
            root: root.to_path_buf(),
        });
    }
    let rel = path.strip_prefix(root).unwrap();
    if rel.as_os_str().is_empty() {
        Ok(PathBuf::from("."))
    } else {
        workspace_relative(rel).map_err(|_e| LocateError::OutsideWorkspaceRoot {
            path: path.to_path_buf(),
            root: root.to_path_buf(),
        })
    }
}
