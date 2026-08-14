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
                callisto_manifests::cargo_package_name(&doc)
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
                let doc: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_str(&content).map_err(|e| {
                        callisto_model::ManifestError::Parse {
                            path: project_root.join("package.json"),
                            format: ManifestFormat::PackageJson,
                            message: e.to_string(),
                        }
                    })?;
                callisto_manifests::npm_package_name(&doc)
                    .ok_or_else(|| callisto_model::ManifestError::MissingField {
                        path: project_root.join("package.json"),
                        field: "name",
                    })?
                    .to_string()
            }
            Ecosystem::Pypi => {
                let pyproject_toml = abs.join("pyproject.toml");
                let content = std::fs::read_to_string(&pyproject_toml).map_err(|e| {
                    callisto_model::ManifestError::Read {
                        path: project_root.join("pyproject.toml"),
                        message: e.to_string(),
                    }
                })?;
                let doc: toml_edit::DocumentMut =
                    content.parse().map_err(|e: toml_edit::TomlError| {
                        callisto_model::ManifestError::Parse {
                            path: project_root.join("pyproject.toml"),
                            format: ManifestFormat::PyprojectToml,
                            message: e.to_string(),
                        }
                    })?;
                callisto_manifests::python_package_name(&doc)
                    .ok_or_else(|| callisto_model::ManifestError::MissingField {
                        path: project_root.join("pyproject.toml"),
                        field: "project.name / tool.poetry.name / tool.flit.metadata.module",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_cargo_package_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let resolver = IdentityResolver::new(dir.path()).unwrap();
        let id = resolver
            .resolve(std::path::Path::new("."), Ecosystem::Cargo)
            .unwrap();
        assert_eq!(id.name(), "my-crate");
    }

    /// A `Cargo.toml` using `version.workspace = true` (real-world common
    /// case) must still resolve by name alone -- a package's *name* is
    /// never workspace-inherited in Cargo, so this must succeed without
    /// any `WorkspaceInheritance` context, which `IdentityResolver` (used
    /// from `callisto-moon`'s WASM PDK entry points, which have no such
    /// context available) deliberately never builds.
    #[test]
    fn resolves_cargo_package_name_with_workspace_inherited_version() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"inheriting-crate\"\nversion.workspace = true\nedition.workspace = true\n",
        )
        .unwrap();
        let resolver = IdentityResolver::new(dir.path()).unwrap();
        let id = resolver
            .resolve(std::path::Path::new("."), Ecosystem::Cargo)
            .unwrap();
        assert_eq!(id.name(), "inheriting-crate");
    }

    #[test]
    fn resolves_npm_package_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"name":"my-pkg","version":"1.0.0"}"#,
        )
        .unwrap();
        let resolver = IdentityResolver::new(dir.path()).unwrap();
        let id = resolver
            .resolve(std::path::Path::new("."), Ecosystem::Npm)
            .unwrap();
        assert_eq!(id.name(), "my-pkg");
    }

    #[test]
    fn resolves_pypi_package_name_pep621() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"my-lib\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let resolver = IdentityResolver::new(dir.path()).unwrap();
        let id = resolver
            .resolve(std::path::Path::new("."), Ecosystem::Pypi)
            .unwrap();
        assert_eq!(id.name(), "my-lib");
    }

    #[test]
    fn resolves_pypi_package_name_poetry_fallback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.poetry]\nname = \"my-poetry-lib\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let resolver = IdentityResolver::new(dir.path()).unwrap();
        let id = resolver
            .resolve(std::path::Path::new("."), Ecosystem::Pypi)
            .unwrap();
        assert_eq!(id.name(), "my-poetry-lib");
    }

    /// Before the shared-extractor refactor, IdentityResolver's Pypi branch
    /// only checked PEP 621 then Poetry -- unlike
    /// `PyprojectToml::package_name()`, which also falls back to Flit's
    /// `[tool.flit.metadata].module`. A Flit-based Python package could be
    /// resolved via the Manifest trait but not via IdentityResolver. The
    /// shared `python_package_name` extractor closes this gap as a
    /// side effect of removing the duplication.
    #[test]
    fn resolves_pypi_package_name_flit_fallback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.flit.metadata]\nmodule = \"my_flit_lib\"\n",
        )
        .unwrap();
        let resolver = IdentityResolver::new(dir.path()).unwrap();
        let id = resolver
            .resolve(std::path::Path::new("."), Ecosystem::Pypi)
            .unwrap();
        assert_eq!(id.name(), "my_flit_lib");
    }

    #[test]
    fn resolve_errors_when_manifest_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = IdentityResolver::new(dir.path()).unwrap();
        let result = resolver.resolve(std::path::Path::new("."), Ecosystem::Cargo);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_errors_when_name_field_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        let resolver = IdentityResolver::new(dir.path()).unwrap();
        let result = resolver.resolve(std::path::Path::new("."), Ecosystem::Cargo);
        assert!(result.is_err());
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

    /// Look up a package by its native (manifest-declared) name.
    ///
    /// First tries the exact `(eco, name)` key (same-ecosystem lookup).  If
    /// that misses — which happens when a package in one ecosystem depends on a
    /// package in a *different* ecosystem by bare name (e.g. an npm package
    /// listing a cargo crate as a dependency) — it falls back to scanning all
    /// ecosystems for `name`.
    ///
    /// If the fallback finds **exactly one** match the cross-ecosystem package
    /// is returned.  If it finds **more than one** (i.e. two different
    /// ecosystems both have a package called `name`), `None` is returned to
    /// prevent silent misresolution; callers that hold a `&mut Vec<Diagnostic>`
    /// should use [`Self::resolve_native_with_fallback`] instead to get a diagnostic
    /// in that case.
    pub fn resolve_native(&self, eco: Ecosystem, name: &str) -> Option<&PackageId> {
        // Fast path: exact ecosystem match.
        if let Some(id) = self.native.get(&(eco, name.to_string())) {
            return Some(id);
        }

        // Cross-ecosystem fallback.
        let mut candidates: Vec<&PackageId> = self
            .native
            .iter()
            .filter(|((e, n), _)| *e != eco && n == name)
            .map(|(_, id)| id)
            .collect();

        // Deduplicate by pointer identity (multiple entries for the same ID
        // across different ecosystems, e.g. a package that is both cargo and
        // npm, should not be treated as ambiguous).
        candidates.dedup_by(|a, b| a == b);

        match candidates.len() {
            1 => Some(candidates[0]),
            // 0 = not found; >1 = true ambiguity → caller should diagnose.
            _ => None,
        }
    }

    /// Like [`Self::resolve_native`] but pushes a [`callisto_model::Diagnostic`] when
    /// cross-ecosystem ambiguity is detected (two packages with the same bare
    /// name in different ecosystems).
    pub fn resolve_native_with_fallback<'a>(
        &'a self,
        eco: Ecosystem,
        name: &str,
        diagnostics: &mut Vec<callisto_model::Diagnostic>,
    ) -> Option<&'a PackageId> {
        // Fast path: exact ecosystem match.
        if let Some(id) = self.native.get(&(eco, name.to_string())) {
            return Some(id);
        }

        // Cross-ecosystem fallback: collect unique IDs from other ecosystems.
        let mut candidates: Vec<(Ecosystem, &PackageId)> = self
            .native
            .iter()
            .filter(|((e, n), _)| *e != eco && n == name)
            .map(|((e, _), id)| (*e, id))
            .collect();
        candidates.dedup_by(|a, b| a.1 == b.1);

        match candidates.len() {
            0 => None,
            1 => Some(candidates[0].1),
            _ => {
                // True ambiguity: two packages with different ecosystems share
                // the same bare name.  Emit a diagnostic and return None to
                // avoid silent misresolution.
                let candidate_names: Vec<String> = candidates
                    .iter()
                    .map(|(e, id)| format!("{}:{}", e.prefix(), id.name()))
                    .collect();
                diagnostics.push(callisto_model::Diagnostic {
                    code: callisto_model::DiagnosticCode::UnknownPackage,
                    severity: callisto_model::DiagnosticSeverity::Warning,
                    message: format!(
                        "dependency name `{}` is ambiguous across ecosystems: {}; \
                         add an ecosystem prefix (e.g. `cargo:{}`) to disambiguate",
                        name,
                        candidate_names.join(", "),
                        name,
                    ),
                    package: None,
                    path: None,
                    escalated_by: None,
                    governed_by: None,
                });
                None
            }
        }
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
