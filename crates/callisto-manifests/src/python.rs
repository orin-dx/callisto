use std::fs;
use std::path::{Path, PathBuf};

use callisto_model::{
    DepKind, DepSpec, DependencyEntry, Ecosystem, ManifestDecl, ManifestError, ManifestFormat,
    ManifestRole, PublishTarget, Version, VersionGrammar, VersionReq,
};
use toml_edit::value;

use crate::atomic::atomic_write;
use crate::{Manifest, OpenContext};

/// A `pyproject.toml` manifest editor supporting PEP 621, Poetry, and Flit schemas with 100% CST comment preservation.
pub struct PyprojectToml {
    path: PathBuf,
    absolute: PathBuf,
    role: ManifestRole,
    document: toml_edit::DocumentMut,
}

impl PyprojectToml {
    pub fn open(decl: &ManifestDecl, ctx: &OpenContext<'_>) -> Result<Self, ManifestError> {
        let rel_path = decl.path.clone();
        let abs_path = ctx.workspace_root.join(&rel_path);

        let content = fs::read_to_string(&abs_path).map_err(|e| ManifestError::Read {
            path: rel_path.clone(),
            message: e.to_string(),
        })?;

        let clean_content = content.strip_prefix('\u{FEFF}').unwrap_or(&content);

        let doc: toml_edit::DocumentMut =
            clean_content
                .parse()
                .map_err(|e: toml_edit::TomlError| ManifestError::Parse {
                    path: rel_path.clone(),
                    format: ManifestFormat::PyprojectToml,
                    message: e.to_string(),
                })?;

        Ok(Self {
            path: rel_path,
            absolute: abs_path,
            role: decl.role.clone(),
            document: doc,
        })
    }
}

impl Manifest for PyprojectToml {
    fn path(&self) -> &Path {
        &self.path
    }

    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Pypi
    }

    fn role(&self) -> ManifestRole {
        self.role.clone()
    }

    fn package_name(&self) -> Result<String, ManifestError> {
        // 1. PEP 621 [project].name
        if let Some(name) = self
            .document
            .get("project")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
        {
            return Ok(name.to_string());
        }

        // 2. Poetry [tool.poetry].name
        if let Some(name) = self
            .document
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
        {
            return Ok(name.to_string());
        }

        // 3. Flit [tool.flit.metadata].module
        if let Some(module) = self
            .document
            .get("tool")
            .and_then(|t| t.get("flit"))
            .and_then(|f| f.get("metadata"))
            .and_then(|m| m.get("module"))
            .and_then(|n| n.as_str())
        {
            return Ok(module.to_string());
        }

        Err(ManifestError::MissingField {
            path: self.path.clone(),
            field: "project.name / tool.poetry.name",
        })
    }

    fn current_version(&self) -> Result<Version, ManifestError> {
        // 1. PEP 621 [project].version
        let raw_ver = self
            .document
            .get("project")
            .and_then(|p| p.get("version"))
            .and_then(|v| v.as_str())
            // 2. Poetry [tool.poetry].version
            .or_else(|| {
                self.document
                    .get("tool")
                    .and_then(|t| t.get("poetry"))
                    .and_then(|p| p.get("version"))
                    .and_then(|v| v.as_str())
            })
            // 3. Flit [tool.flit.metadata].version
            .or_else(|| {
                self.document
                    .get("tool")
                    .and_then(|t| t.get("flit"))
                    .and_then(|f| f.get("metadata"))
                    .and_then(|m| m.get("version"))
                    .and_then(|v| v.as_str())
            });

        let raw = raw_ver.ok_or_else(|| ManifestError::MissingField {
            path: self.path.clone(),
            field: "project.version / tool.poetry.version",
        })?;

        Version::parse(raw, VersionGrammar::Pep440).map_err(|e| ManifestError::InvalidVersion {
            path: self.path.clone(),
            raw: raw.to_string(),
            source: e,
        })
    }

    fn write_version(&mut self, v: &Version) -> Result<(), ManifestError> {
        let new_ver = v.render();

        let set_with_decor = |table: &mut toml_edit::Item, key: &str| {
            if let Some(existing) = table.get_mut(key).and_then(|i| i.as_value_mut()) {
                let decor = existing.decor().clone();
                let mut new_val = value(new_ver);
                if let Some(v_mut) = new_val.as_value_mut() {
                    *v_mut.decor_mut() = decor;
                }
                table[key] = new_val;
            } else {
                table[key] = value(new_ver);
            }
        };

        if let Some(proj) = self.document.get_mut("project") {
            set_with_decor(proj, "version");
        } else if let Some(poetry) = self
            .document
            .get_mut("tool")
            .and_then(|t| t.get_mut("poetry"))
        {
            set_with_decor(poetry, "version");
        } else if let Some(flit) = self
            .document
            .get_mut("tool")
            .and_then(|t| t.get_mut("flit"))
            .and_then(|f| f.get_mut("metadata"))
        {
            set_with_decor(flit, "version");
        } else {
            self.document["project"]["version"] = value(new_ver);
        }

        let content = self.document.to_string();
        atomic_write(&self.absolute, &content).map_err(|e| ManifestError::Write {
            path: self.path.clone(),
            message: e.to_string(),
        })
    }

    fn iter_dependencies(&self) -> Box<dyn Iterator<Item = DependencyEntry> + '_> {
        let mut entries = Vec::new();

        // 1. PEP 621 [project].dependencies (array of requirement strings)
        if let Some(deps) = self
            .document
            .get("project")
            .and_then(|p| p.get("dependencies"))
            .and_then(|d| d.as_array())
        {
            for item in deps {
                if let Some(full_req_str) = item.as_str() {
                    let spec_part = full_req_str
                        .split(';')
                        .next()
                        .unwrap_or(full_req_str)
                        .trim();
                    let op_idx = spec_part.find(&['<', '>', '=', '!', '~'][..]);
                    let (pkg_part, req_str) = match op_idx {
                        Some(idx) => (&spec_part[..idx], &spec_part[idx..]),
                        None => (spec_part, "*"),
                    };
                    let pkg_name = pkg_part.split('[').next().unwrap_or(pkg_part).trim();

                    if !pkg_name.is_empty() {
                        let parsed_req_str = if req_str == "*" { ">=0.0.0" } else { req_str };
                        if let Ok(spec) = VersionReq::parse(parsed_req_str, Ecosystem::Pypi) {
                            entries.push(DependencyEntry {
                                name: pkg_name.to_string(),
                                inherited: false,
                                kind: DepKind::Runtime,
                                spec: DepSpec::Range(spec, req_str.to_string()),
                            });
                        }
                    }
                }
            }
        }

        // 2. Poetry [tool.poetry.dependencies]
        if let Some(table) = self
            .document
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("dependencies"))
            .and_then(|d| d.as_table_like())
        {
            for (name, item) in table.iter() {
                if name == "python" {
                    continue;
                }
                let raw_req = match item {
                    toml_edit::Item::Value(toml_edit::Value::String(s)) => Some(s.value().as_str()),
                    toml_edit::Item::Value(toml_edit::Value::InlineTable(t)) => {
                        t.get("version").and_then(|v| v.as_str())
                    }
                    _ => None,
                };

                if let Some(req_str) = raw_req {
                    if let Ok(spec) = VersionReq::parse(req_str, Ecosystem::Pypi) {
                        entries.push(DependencyEntry {
                            name: name.to_string(),
                            inherited: false,
                            kind: DepKind::Runtime,
                            spec: DepSpec::Range(spec, req_str.to_string()),
                        });
                    }
                }
            }
        }

        Box::new(entries.into_iter())
    }

    fn update_dependency_spec(
        &mut self,
        name: &str,
        _kind: DepKind,
        new: DepSpec,
    ) -> Result<(), ManifestError> {
        let new_spec_str = match new {
            DepSpec::Range(req, _) => req.render().to_string(),
            DepSpec::Exact(v) => format!("=={}", v.render()),
            _ => return Ok(()),
        };

        let mut updated = false;

        // 1. Update PEP 621 [project].dependencies array entries if present
        if let Some(deps) = self
            .document
            .get_mut("project")
            .and_then(|p| p.get_mut("dependencies"))
            .and_then(|d| d.as_array_mut())
        {
            for idx in 0..deps.len() {
                if let Some(full_req) = deps.get(idx).and_then(|item| item.as_str()) {
                    let spec_part = full_req.split(';').next().unwrap_or(full_req).trim();
                    let op_idx = spec_part.find(&['<', '>', '=', '!', '~'][..]);
                    let pkg_part = match op_idx {
                        Some(i) => &spec_part[..i],
                        None => spec_part,
                    };
                    let pkg_name = pkg_part.split('[').next().unwrap_or(pkg_part).trim();
                    if pkg_name.eq_ignore_ascii_case(name) {
                        let extras = if pkg_part.contains('[') {
                            &pkg_part[pkg_part.find('[').unwrap()..]
                        } else {
                            ""
                        };
                        let marker = if full_req.contains(';') {
                            format!(";{}", full_req.split(';').nth(1).unwrap_or(""))
                        } else {
                            String::new()
                        };
                        let formatted_spec = if new_spec_str.starts_with(['<', '>', '=', '!', '~'])
                        {
                            new_spec_str.clone()
                        } else {
                            format!(">={new_spec_str}")
                        };
                        deps.replace(idx, format!("{pkg_name}{extras}{formatted_spec}{marker}"));
                        updated = true;
                        break;
                    }
                }
            }
        }

        // 2. Update Poetry dependencies if present (preserving inline tables if used)
        if let Some(table) = self
            .document
            .get_mut("tool")
            .and_then(|t| t.get_mut("poetry"))
            .and_then(|p| p.get_mut("dependencies"))
            .and_then(|d| d.as_table_like_mut())
        {
            if let Some(existing) = table.get_mut(name) {
                match existing {
                    toml_edit::Item::Value(toml_edit::Value::InlineTable(ref mut inline)) => {
                        inline.insert("version", toml_edit::Value::from(new_spec_str.as_str()));
                    }
                    _ => {
                        table.insert(name, value(new_spec_str));
                    }
                }
                updated = true;
            }
        }

        if updated {
            let content = self.document.to_string();
            atomic_write(&self.absolute, &content).map_err(|e| ManifestError::Write {
                path: self.path.clone(),
                message: e.to_string(),
            })?;
        }

        Ok(())
    }

    fn publish_targets(&self) -> Vec<PublishTarget> {
        if !self.is_publishable() {
            return vec![PublishTarget::None];
        }
        vec![PublishTarget::Pypi { index: None }]
    }

    fn update_optional_dependencies(
        &mut self,
        _updates: &[(String, Version)],
    ) -> Result<(), ManifestError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::ManifestFormat;
    use tempfile::tempdir;

    #[test]
    fn parses_pep621_pyproject_and_updates_version_with_comment_preservation() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");

        let input_content = r#"[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

# Main project metadata
[project]
name = "my-python-lib" # The package name
version = "0.3.1" # Current release version
description = "A polyglot library"
dependencies = [
    "requests>=2.28.0",
]
"#;

        fs::write(&pyproject_path, input_content).unwrap();

        let decl = ManifestDecl {
            path: PathBuf::from("pyproject.toml"),
            role: ManifestRole::Canonical,
            format: ManifestFormat::PyprojectToml,
        };

        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
        assert_eq!(manifest.package_name().unwrap(), "my-python-lib");

        let current_v = manifest.current_version().unwrap();
        assert_eq!(current_v.render(), "0.3.1");

        let new_v = Version::parse("0.3.2", VersionGrammar::Pep440).unwrap();
        manifest.write_version(&new_v).unwrap();

        let updated_content = fs::read_to_string(&pyproject_path).unwrap();
        assert!(updated_content.contains("version = \"0.3.2\""));
        assert!(updated_content.contains("# Main project metadata"));
        assert!(updated_content.contains("# Current release version"));
    }

    #[test]
    fn handles_utf8_bom_pyproject_toml() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");

        let input_content = "\u{FEFF}[project]\nname = \"bom-lib\"\nversion = \"1.0.0\"\n";
        fs::write(&pyproject_path, input_content).unwrap();

        let decl = ManifestDecl {
            path: PathBuf::from("pyproject.toml"),
            role: ManifestRole::Canonical,
            format: ManifestFormat::PyprojectToml,
        };

        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let manifest = PyprojectToml::open(&decl, &ctx).unwrap();
        assert_eq!(manifest.package_name().unwrap(), "bom-lib");
        assert_eq!(manifest.current_version().unwrap().render(), "1.0.0");
    }

    #[test]
    fn parses_pep508_extras_and_markers() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");

        let input_content = r#"[project]
name = "complex-deps"
version = "0.1.0"
dependencies = [
    "requests[security]>=2.28.0; os_name == 'posix'",
    "urllib3<2.0.0",
]
"#;
        fs::write(&pyproject_path, input_content).unwrap();

        let decl = ManifestDecl {
            path: PathBuf::from("pyproject.toml"),
            role: ManifestRole::Canonical,
            format: ManifestFormat::PyprojectToml,
        };

        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let manifest = PyprojectToml::open(&decl, &ctx).unwrap();
        let deps: Vec<_> = manifest.iter_dependencies().collect();
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "requests");
        assert_eq!(deps[1].name, "urllib3");
    }

    #[test]
    fn updates_pep621_dependencies_array() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");

        let input_content = r#"[project]
name = "my-app"
version = "0.1.0"
dependencies = [
    "my-lib>=0.3.0",
]
"#;
        fs::write(&pyproject_path, input_content).unwrap();

        let decl = ManifestDecl {
            path: PathBuf::from("pyproject.toml"),
            role: ManifestRole::Canonical,
            format: ManifestFormat::PyprojectToml,
        };

        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
        let req = VersionReq::parse(">=0.3.2", Ecosystem::Pypi).unwrap();
        manifest
            .update_dependency_spec(
                "my-lib",
                DepKind::Runtime,
                DepSpec::Range(req, ">=0.3.2".to_string()),
            )
            .unwrap();

        let updated_content = fs::read_to_string(&pyproject_path).unwrap();
        assert!(updated_content.contains("my-lib>=0.3.2"));
    }
}
