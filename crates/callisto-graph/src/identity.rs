use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use callisto_model::{Ecosystem, ManifestFormat, PackageId};

use crate::error::GraphError;

pub struct IdentityResolver {
    workspace_root: PathBuf,
}

impl IdentityResolver {
    pub fn new(workspace_root: &Path) -> Result<Self, GraphError> {
        Ok(IdentityResolver {
            workspace_root: workspace_root.to_path_buf(),
        })
    }

    pub fn resolve(
        &self,
        project_root: &Path,
        ecosystem: Ecosystem,
    ) -> Result<PackageId, GraphError> {
        let abs = self.workspace_root.join(project_root);
        let name = match ecosystem {
            Ecosystem::Cargo => {
                let cargo_toml = abs.join("Cargo.toml");
                let content = std::fs::read_to_string(&cargo_toml).map_err(|e| {
                    callisto_model::ManifestError::Read {
                        path: project_root.join("Cargo.toml"),
                        message: e.to_string(),
                    }
                })?;
                let doc: toml_edit::DocumentMut =
                    content.parse().map_err(|e: toml_edit::TomlError| {
                        callisto_model::ManifestError::Parse {
                            path: project_root.join("Cargo.toml"),
                            format: ManifestFormat::CargoToml,
                            message: e.to_string(),
                        }
                    })?;
                doc.get("package")
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                    .ok_or_else(|| callisto_model::ManifestError::MissingField {
                        path: project_root.join("Cargo.toml"),
                        field: "package.name",
                    })?
                    .to_string()
            }
            Ecosystem::Npm => {
                let pkg_json = abs.join("package.json");
                let content = std::fs::read_to_string(&pkg_json).map_err(|e| {
                    callisto_model::ManifestError::Read {
                        path: project_root.join("package.json"),
                        message: e.to_string(),
                    }
                })?;
                let val: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
                    callisto_model::ManifestError::Parse {
                        path: project_root.join("package.json"),
                        format: ManifestFormat::PackageJson,
                        message: e.to_string(),
                    }
                })?;
                val.get("name")
                    .and_then(|n| n.as_str())
                    .ok_or_else(|| callisto_model::ManifestError::MissingField {
                        path: project_root.join("package.json"),
                        field: "name",
                    })?
                    .to_string()
            }
            _ => {
                return Err(GraphError::AmbiguousName {
                    name: "unsupported ecosystem".to_string(),
                    candidates: Vec::new(),
                });
            }
        };

        PackageId::parse(&name).map_err(|_err| GraphError::AmbiguousName {
            name: name.clone(),
            candidates: Vec::new(),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct IdentityIndex {
    pub bare: BTreeMap<String, PackageId>,
    pub prefixed: BTreeMap<(Ecosystem, String), PackageId>,
    pub native: BTreeMap<(Ecosystem, String), PackageId>,
    pub platform: BTreeMap<String, (PackageId, PathBuf)>,
}

impl IdentityIndex {
    pub fn resolve_human(
        &self,
        name: &str,
        siblings: &[PackageId],
    ) -> Result<PackageId, GraphError> {
        if let Ok(id) = PackageId::parse(name) {
            if self.bare.values().any(|v| v == &id) || self.prefixed.values().any(|v| v == &id) {
                return Ok(id);
            }
        }

        if let Some(id) = self.bare.get(name) {
            return Ok(id.clone());
        }

        let mut candidates = Vec::new();
        for ((_eco, n), id) in &self.prefixed {
            if n == name {
                candidates.push(id.clone());
            }
        }

        if candidates.len() == 1 {
            return Ok(candidates[0].clone());
        }

        if candidates.len() > 1 && !siblings.is_empty() {
            for sib in siblings {
                for cand in &candidates {
                    if cand.ecosystem() == sib.ecosystem() {
                        return Ok(cand.clone());
                    }
                }
            }
        }

        if candidates.is_empty() {
            Err(GraphError::UnknownPackage {
                id: PackageId::parse(name).unwrap_or_else(|_| PackageId::Bare(name.to_string())),
            })
        } else {
            Err(GraphError::AmbiguousName {
                name: name.to_string(),
                candidates,
            })
        }
    }

    pub fn resolve_native(&self, eco: Ecosystem, name: &str) -> Option<&PackageId> {
        self.native.get(&(eco, name.to_string()))
    }

    pub fn native_name(&self, id: &PackageId, eco: Ecosystem) -> Option<&str> {
        for ((e, name), registered_id) in &self.native {
            if e == &eco && registered_id == id {
                return Some(name.as_str());
            }
        }
        None
    }

    pub fn native_names(&self, id: &PackageId) -> impl Iterator<Item = (Ecosystem, &str)> {
        let mut results = Vec::new();
        for ((e, name), registered_id) in &self.native {
            if registered_id == id {
                results.push((*e, name.as_str()));
            }
        }
        results.into_iter()
    }

    pub fn display_form(&self, id: &PackageId) -> String {
        id.display_name()
    }

    pub fn platforms_of(&self, owner: &PackageId) -> impl Iterator<Item = (&str, &Path)> {
        let mut results = Vec::new();
        for (name, (plat_owner, path)) in &self.platform {
            if plat_owner == owner {
                results.push((name.as_str(), path.as_path()));
            }
        }
        results.into_iter()
    }
}
