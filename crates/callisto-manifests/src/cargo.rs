use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use callisto_model::{
    workspace_relative, ApplyPermit, DepKind, DepSpec, DependencyEntry, Ecosystem, ManifestDecl,
    ManifestError, ManifestFormat, ManifestRole, Version, VersionGrammar, VersionReq,
};

use crate::{Manifest, OpenContext};

pub struct CargoToml {
    path: PathBuf,
    absolute: PathBuf,
    workspace_root: PathBuf,
    role: ManifestRole,
    document: toml_edit::DocumentMut,
    inherited_deps: HashSet<(DepKind, String)>,
    inherited_version: bool,
    inheritance: Option<Arc<WorkspaceInheritance>>,
    has_bom: bool,
}

impl CargoToml {
    pub fn open(decl: &ManifestDecl, ctx: &OpenContext<'_>) -> Result<Self, ManifestError> {
        let rel_path = decl.path.clone();
        let abs_path = ctx.workspace_root.join(&rel_path);

        let content = fs::read_to_string(&abs_path).map_err(|e| ManifestError::Read {
            path: rel_path.clone(),
            message: e.to_string(),
        })?;

        let has_bom = content.starts_with('\u{FEFF}');
        let clean_content = content.strip_prefix('\u{FEFF}').unwrap_or(&content);

        let doc: toml_edit::DocumentMut =
            clean_content
                .parse()
                .map_err(|e: toml_edit::TomlError| ManifestError::Parse {
                    path: rel_path.clone(),
                    format: ManifestFormat::CargoToml,
                    message: e.to_string(),
                })?;

        let inherited_version = doc
            .get("package")
            .and_then(|p| p.get("version"))
            .and_then(|v| v.get("workspace"))
            .and_then(|w| w.as_bool())
            == Some(true);

        let mut inherited_deps = HashSet::new();
        for (section_name, kind) in [
            ("dependencies", DepKind::Runtime),
            ("dev-dependencies", DepKind::Dev),
            ("build-dependencies", DepKind::Build),
        ] {
            if let Some(table) = doc.get(section_name).and_then(|t| t.as_table_like()) {
                for (name, item) in table.iter() {
                    let is_inherited = item
                        .as_inline_table()
                        .and_then(|t| t.get("workspace"))
                        .and_then(|w| w.as_bool())
                        == Some(true)
                        || item
                            .as_table()
                            .and_then(|t| t.get("workspace"))
                            .and_then(|w| w.as_bool())
                            == Some(true);
                    if is_inherited {
                        inherited_deps.insert((kind, name.to_string()));
                    }
                }
            }
        }

        let uses_inheritance = inherited_version || !inherited_deps.is_empty();
        if uses_inheritance && ctx.cargo_workspace.is_none() {
            return Err(ManifestError::Read {
                path: rel_path,
                message:
                    "declares .workspace = true but no WorkspaceCargoResolver context was supplied"
                        .to_string(),
            });
        }

        Ok(CargoToml {
            path: rel_path,
            absolute: abs_path,
            workspace_root: ctx.workspace_root.to_path_buf(),
            role: decl.role.clone(),
            document: doc,
            inherited_deps,
            inherited_version,
            inheritance: ctx.cargo_workspace.clone(),
            has_bom,
        })
    }
}

impl Manifest for CargoToml {
    fn persist(&mut self, permit: &ApplyPermit) -> Result<(), ManifestError> {
        let mut text = self.document.to_string();
        if self.has_bom {
            text = format!("\u{FEFF}{}", text);
        }
        crate::atomic::atomic_write(&self.absolute, &text, permit).map_err(|e| {
            ManifestError::Write {
                path: self.path.clone(),
                message: e.to_string(),
            }
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Cargo
    }

    fn is_publishable(&self) -> bool {
        let p = match self.document.get("package") {
            Some(p) => p,
            None => return false,
        };
        if let Some(pub_val) = p.get("publish") {
            if let Some(b) = pub_val.as_bool() {
                return b;
            }
            if let Some(arr) = pub_val.as_array() {
                return !arr.is_empty();
            }
        }
        true
    }

    fn publish_targets(&self) -> Vec<callisto_model::PublishTarget> {
        if !self.is_publishable() {
            vec![callisto_model::PublishTarget::None]
        } else {
            vec![callisto_model::PublishTarget::CratesIo]
        }
    }

    fn role(&self) -> ManifestRole {
        self.role.clone()
    }

    fn package_name(&self) -> Result<String, ManifestError> {
        let name = self
            .document
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .ok_or_else(|| ManifestError::MissingField {
                path: self.path.clone(),
                field: "package.name",
            })?;
        Ok(name.to_string())
    }

    fn current_version(&self) -> Result<Version, ManifestError> {
        if self.inherited_version {
            if let Some(ref inh) = self.inheritance {
                if let Some(ref v) = inh.version {
                    return Ok(v.clone());
                }
            }
            return Err(ManifestError::WorkspaceInherited {
                path: self.path.clone(),
                key: "version".to_string(),
            });
        }

        let raw = self
            .document
            .get("package")
            .and_then(|p| p.get("version"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ManifestError::MissingField {
                path: self.path.clone(),
                field: "package.version",
            })?;

        Version::parse(raw, VersionGrammar::SemVer).map_err(|source| {
            ManifestError::InvalidVersion {
                path: self.path.clone(),
                raw: raw.to_string(),
                source,
            }
        })
    }

    fn write_version(&mut self, v: &Version, _permit: &ApplyPermit) -> Result<(), ManifestError> {
        if self.inherited_version {
            // The member uses `version.workspace = true` (or `version = { workspace = true }`).
            // Routing the bump to [workspace.package] would silently change the version for
            // every other workspace member. Instead, write an explicit pinned version directly
            // to this member's [package] section, replacing the workspace-inherited entry with
            // a standalone string value.
            let pkg = self
                .document
                .get_mut("package")
                .and_then(|p| p.as_table_mut())
                .ok_or_else(|| ManifestError::MissingField {
                    path: self.path.clone(),
                    field: "package",
                })?;
            pkg.insert("version", toml_edit::value(v.render()));
            self.inherited_version = false;
            return Ok(());
        }

        let pkg = self
            .document
            .get_mut("package")
            .and_then(|p| p.as_table_mut())
            .ok_or_else(|| ManifestError::MissingField {
                path: self.path.clone(),
                field: "package",
            })?;

        if let Some(item) = pkg.get_mut("version") {
            if let Some(val) = item.as_value_mut() {
                let decor = val.decor().clone();
                let mut new_val = toml_edit::Value::from(v.render());
                *new_val.decor_mut() = decor;
                *val = new_val;
            } else {
                pkg.insert("version", toml_edit::value(v.render()));
            }
        } else {
            pkg.insert("version", toml_edit::value(v.render()));
        }
        Ok(())
    }

    fn iter_dependencies(&self) -> Box<dyn Iterator<Item = DependencyEntry> + '_> {
        let mut entries = Vec::new();

        for (section_name, section_kind) in [
            ("dependencies", DepKind::Runtime),
            ("dev-dependencies", DepKind::Dev),
            ("build-dependencies", DepKind::Build),
        ] {
            if let Some(table) = self
                .document
                .get(section_name)
                .and_then(|t| t.as_table_like())
            {
                for (name, item) in table.iter() {
                    let is_inherited = self
                        .inherited_deps
                        .contains(&(section_kind, name.to_string()));
                    let mut kind = section_kind;

                    let spec = if is_inherited {
                        if let Some(ref inh) = self.inheritance {
                            if let Some(inherited_dep) = inh.inherited(name) {
                                inherited_dep.spec.clone()
                            } else {
                                DepSpec::Opaque("workspace = true (unresolved)".to_string())
                            }
                        } else {
                            DepSpec::Opaque("workspace = true (unresolved)".to_string())
                        }
                    } else {
                        parse_cargo_dep_item(item, &mut kind)
                    };

                    entries.push(DependencyEntry {
                        name: name.to_string(),
                        kind,
                        spec,
                        inherited: is_inherited,
                    });
                }
            }
        }

        Box::new(entries.into_iter())
    }

    fn update_dependency_spec(
        &mut self,
        name: &str,
        kind: DepKind,
        new: DepSpec,
        permit: &ApplyPermit,
    ) -> Result<(), ManifestError> {
        if kind == DepKind::Peer {
            return Err(ManifestError::DependencyNotFound {
                path: self.path.clone(),
                name: name.to_string(),
                kind,
            });
        }

        if self.inherited_deps.contains(&(kind, name.to_string())) {
            let root_cargo = self.workspace_root.join("Cargo.toml");
            if self.absolute == root_cargo {
                return Err(ManifestError::InvariantViolation {
                    path: self.path.clone(),
                    message: format!(
                        "dependency `{name}` is workspace-inherited but this manifest IS the workspace root (`{}`); refusing to delegate to avoid overwriting the just-written root manifest with a stale in-memory document",
                        root_cargo.display()
                    ),
                });
            }
            let mut ws_res = WorkspaceCargoResolver::load(&root_cargo)?;
            return ws_res.write_dependency(name, new, permit);
        }

        let section_name = match kind {
            DepKind::Runtime | DepKind::Optional => "dependencies",
            DepKind::Dev => "dev-dependencies",
            DepKind::Build => "build-dependencies",
            DepKind::Peer => unreachable!(),
        };

        let table = self
            .document
            .get_mut(section_name)
            .and_then(|t| t.as_table_like_mut())
            .ok_or_else(|| ManifestError::DependencyNotFound {
                path: self.path.clone(),
                name: name.to_string(),
                kind,
            })?;

        let item = table
            .get_mut(name)
            .ok_or_else(|| ManifestError::DependencyNotFound {
                path: self.path.clone(),
                name: name.to_string(),
                kind,
            })?;

        let new_str = new.render();

        if let Some(value) = item.as_value_mut() {
            if value.is_str() {
                let decor = value.decor().clone();
                let mut new_val = toml_edit::Value::from(new_str);
                *new_val.decor_mut() = decor;
                *value = new_val;
            } else if let Some(inline) = value.as_inline_table_mut() {
                if let Some(existing_ver) = inline.get_mut("version") {
                    let decor = existing_ver.decor().clone();
                    let mut new_val = toml_edit::Value::from(new_str);
                    *new_val.decor_mut() = decor;
                    *existing_ver = new_val;
                } else {
                    inline.insert("version", toml_edit::Value::from(new_str));
                }
            }
        } else if let Some(tbl) = item.as_table_mut() {
            let decor = tbl
                .get("version")
                .and_then(|i| i.as_value())
                .map(|v| v.decor().clone());
            tbl.insert("version", toml_edit::value(new_str));
            if let Some(decor) = decor {
                if let Some(new_item) = tbl.get_mut("version").and_then(|i| i.as_value_mut()) {
                    *new_item.decor_mut() = decor;
                }
            }
        }

        self.persist(permit)
    }

    fn update_optional_dependencies(
        &mut self,
        _updates: &[(String, Version)],
        _permit: &ApplyPermit,
    ) -> Result<(), ManifestError> {
        Err(ManifestError::UnsupportedOperation {
            path: self.path.clone(),
            format: ManifestFormat::CargoToml,
            operation: "update_optional_dependencies",
        })
    }
}

fn parse_cargo_dep_item(item: &toml_edit::Item, kind: &mut DepKind) -> DepSpec {
    if let Some(table) = item.as_inline_table() {
        if table.get("optional").and_then(|o| o.as_bool()) == Some(true) {
            *kind = DepKind::Optional;
        }
        if let Some(v_str) = table.get("version").and_then(|v| v.as_str()) {
            return parse_cargo_spec_str(v_str);
        }
    } else if let Some(table) = item.as_table() {
        if table.get("optional").and_then(|o| o.as_bool()) == Some(true) {
            *kind = DepKind::Optional;
        }
        if let Some(v_str) = table.get("version").and_then(|v| v.as_str()) {
            return parse_cargo_spec_str(v_str);
        }
    } else if let Some(v_str) = item.as_str() {
        return parse_cargo_spec_str(v_str);
    }

    DepSpec::Opaque(item.to_string().trim().to_string())
}

fn parse_cargo_spec_str(s: &str) -> DepSpec {
    if crate::common::is_bare_semver(s) {
        if let Ok(v) = Version::parse(s, VersionGrammar::SemVer) {
            return DepSpec::CargoBare(v);
        }
    }
    if let Ok(req) = VersionReq::parse(s, Ecosystem::Cargo) {
        return DepSpec::Range(req, s.to_string());
    }
    DepSpec::Opaque(s.to_string())
}

pub fn round_trip(spec: &DepSpec, target: &Version) -> Option<DepSpec> {
    match spec {
        DepSpec::CargoBare(_) => Some(DepSpec::CargoBare(target.clone())),
        DepSpec::Range(_, original) => {
            if original.contains(',') {
                let parts: Vec<&str> = original.split(',').collect();
                let mut rewritten_parts = Vec::new();
                for part in parts {
                    let part_trimmed = part.trim();
                    let (prefix, rest) = crate::common::split_single_operator_prefix(part_trimmed)?;
                    if prefix.starts_with('>')
                        || prefix.starts_with('=')
                        || prefix.starts_with('^')
                        || prefix.starts_with('~')
                    {
                        let rendered = format!("{prefix}{}", render_at_precision(target, rest));
                        rewritten_parts.push(rendered);
                    } else if prefix.starts_with('<') {
                        // Upper-bound clauses are never rewritten (the clause text
                        // is preserved verbatim), but we must still check whether
                        // the target actually falls within it. If it doesn't, the
                        // rewrite would silently produce an impossible constraint
                        // (e.g. ">=2.1.0, <2.0.0"), so decline with None. If it
                        // does, the upper bound clause is passed through unchanged.
                        let bound = Version::parse(rest, target.grammar()).ok()?;
                        let ordering = target.partial_compare(&bound)?;
                        let exceeds_bound = if prefix == "<=" {
                            ordering == std::cmp::Ordering::Greater
                        } else {
                            ordering != std::cmp::Ordering::Less
                        };
                        if exceeds_bound {
                            return None;
                        }
                        rewritten_parts.push(part_trimmed.to_string());
                    } else {
                        rewritten_parts.push(part_trimmed.to_string());
                    }
                }
                let rendered = rewritten_parts.join(", ");
                let req = VersionReq::parse(&rendered, Ecosystem::Cargo).ok()?;
                return Some(DepSpec::Range(req, rendered));
            }
            let (prefix, rest) = crate::common::split_single_operator_prefix(original)?;
            if rest.contains('*') {
                return None;
            }
            let rendered = format!("{prefix}{}", render_at_precision(target, rest));
            let req = VersionReq::parse(&rendered, Ecosystem::Cargo).ok()?;
            Some(DepSpec::Range(req, rendered))
        }
        _ => None,
    }
}

fn render_at_precision(target: &Version, original_clause: &str) -> String {
    if original_clause.contains('-') || target.is_prerelease() {
        return target.render().to_string();
    }
    let parts: Vec<&str> = original_clause.split('.').collect();
    let maj = target.major().unwrap_or(0);
    let min = target.minor().unwrap_or(0);
    let pat = target.patch().unwrap_or(0);

    match parts.len() {
        1 => format!("{maj}"),
        2 => format!("{maj}.{min}"),
        _ => format!("{maj}.{min}.{pat}"),
    }
}

#[derive(Clone, Debug, Default)]
pub struct WorkspaceInheritance {
    pub root_manifest: PathBuf,
    pub version: Option<Version>,
    pub dependencies: BTreeMap<String, DepSpec>,
}

impl WorkspaceInheritance {
    pub fn inherited(&self, name: &str) -> Option<InheritedDep<'_>> {
        let spec = self.dependencies.get(name)?;
        Some(InheritedDep {
            spec,
            declared_in: &self.root_manifest,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InheritedDep<'a> {
    pub spec: &'a DepSpec,
    pub declared_in: &'a Path,
}

pub struct WorkspaceCargoResolver {
    root_path: PathBuf,
    absolute_path: PathBuf,
    document: toml_edit::DocumentMut,
}

impl WorkspaceCargoResolver {
    pub fn load(root_manifest_path: &Path) -> Result<Self, ManifestError> {
        let rel_path = if root_manifest_path.is_absolute() {
            root_manifest_path
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("Cargo.toml"))
        } else {
            workspace_relative(root_manifest_path).map_err(|e| ManifestError::Read {
                path: root_manifest_path.to_path_buf(),
                message: e.to_string(),
            })?
        };
        let content = fs::read_to_string(root_manifest_path).map_err(|e| ManifestError::Read {
            path: rel_path.clone(),
            message: e.to_string(),
        })?;

        let doc: toml_edit::DocumentMut =
            content
                .parse()
                .map_err(|e: toml_edit::TomlError| ManifestError::Parse {
                    path: rel_path.clone(),
                    format: ManifestFormat::CargoToml,
                    message: e.to_string(),
                })?;

        Ok(WorkspaceCargoResolver {
            root_path: rel_path,
            absolute_path: root_manifest_path.to_path_buf(),
            document: doc,
        })
    }

    pub fn inheritance(&self) -> Result<WorkspaceInheritance, ManifestError> {
        let version = self
            .document
            .get("workspace")
            .and_then(|w| w.get("package"))
            .and_then(|p| p.get("version"))
            .and_then(|v| v.as_str())
            .and_then(|s| Version::parse(s, VersionGrammar::SemVer).ok());

        let mut dependencies = BTreeMap::new();
        if let Some(table) = self
            .document
            .get("workspace")
            .and_then(|w| w.get("dependencies"))
            .and_then(|d| d.as_table_like())
        {
            for (name, item) in table.iter() {
                let mut dummy_kind = DepKind::Runtime;
                let spec = parse_cargo_dep_item(item, &mut dummy_kind);
                dependencies.insert(name.to_string(), spec);
            }
        }

        Ok(WorkspaceInheritance {
            root_manifest: self.root_path.clone(),
            version,
            dependencies,
        })
    }

    /// Returns the `[workspace.package] version` currently on disk, or `None`
    /// when no such field exists (e.g. the workspace does not declare a shared
    /// version). Used by `apply_version_plan`'s idempotency guard.
    pub fn workspace_version(&self) -> Result<Option<Version>, ManifestError> {
        let raw = match self
            .document
            .get("workspace")
            .and_then(|w| w.get("package"))
            .and_then(|p| p.get("version"))
            .and_then(|v| v.as_str())
        {
            Some(s) => s,
            None => return Ok(None),
        };
        Version::parse(raw, VersionGrammar::SemVer)
            .map(Some)
            .map_err(|e| ManifestError::Parse {
                path: self.root_path.clone(),
                format: ManifestFormat::CargoToml,
                message: format!("invalid [workspace.package] version: {e}"),
            })
    }

    pub fn write_version(
        &mut self,
        v: &Version,
        permit: &ApplyPermit,
    ) -> Result<(), ManifestError> {
        let ws = self
            .document
            .get_mut("workspace")
            .and_then(|w| w.as_table_like_mut())
            .ok_or_else(|| ManifestError::MissingField {
                path: self.root_path.clone(),
                field: "workspace",
            })?;

        let pkg = ws
            .entry("package")
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()
            .ok_or_else(|| ManifestError::MissingField {
                path: self.root_path.clone(),
                field: "workspace.package",
            })?;

        if let Some(item) = pkg.get_mut("version") {
            if let Some(val) = item.as_value_mut() {
                let decor = val.decor().clone();
                let mut new_val = toml_edit::Value::from(v.render());
                *new_val.decor_mut() = decor;
                *val = new_val;
            } else {
                pkg.insert("version", toml_edit::value(v.render()));
            }
        } else {
            pkg.insert("version", toml_edit::value(v.render()));
        }
        self.persist(permit)
    }

    pub fn write_dependency(
        &mut self,
        name: &str,
        new: DepSpec,
        permit: &ApplyPermit,
    ) -> Result<(), ManifestError> {
        let ws = self
            .document
            .get_mut("workspace")
            .and_then(|w| w.as_table_mut())
            .ok_or_else(|| ManifestError::MissingField {
                path: self.root_path.clone(),
                field: "workspace",
            })?;

        let deps = ws
            .entry("dependencies")
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_like_mut()
            .ok_or_else(|| ManifestError::MissingField {
                path: self.root_path.clone(),
                field: "workspace.dependencies",
            })?;

        let item = deps
            .get_mut(name)
            .ok_or_else(|| ManifestError::DependencyNotFound {
                path: self.root_path.clone(),
                name: name.to_string(),
                kind: DepKind::Runtime,
            })?;

        let new_str = new.render();
        if let Some(value) = item.as_value_mut() {
            if value.is_str() {
                let decor = value.decor().clone();
                let mut new_val = toml_edit::Value::from(new_str);
                *new_val.decor_mut() = decor;
                *value = new_val;
            } else if let Some(inline) = value.as_inline_table_mut() {
                let decor = inline.get("version").map(|v| v.decor().clone());
                inline.insert("version", toml_edit::Value::from(new_str));
                if let Some(decor) = decor {
                    if let Some(new_value) = inline.get_mut("version") {
                        *new_value.decor_mut() = decor;
                    }
                }
            }
        } else if let Some(tbl) = item.as_table_mut() {
            let decor = tbl
                .get("version")
                .and_then(|i| i.as_value())
                .map(|v| v.decor().clone());
            tbl.insert("version", toml_edit::value(new_str));
            if let Some(decor) = decor {
                if let Some(new_item) = tbl.get_mut("version").and_then(|i| i.as_value_mut()) {
                    *new_item.decor_mut() = decor;
                }
            }
        }

        self.persist(permit)
    }

    fn persist(&mut self, permit: &ApplyPermit) -> Result<(), ManifestError> {
        let text = self.document.to_string();
        crate::atomic::atomic_write(&self.absolute_path, &text, permit).map_err(|e| {
            ManifestError::Write {
                path: self.root_path.clone(),
                message: e.to_string(),
            }
        })
    }
}

#[cfg(test)]
mod tests {

    /// Tests exercise the write primitives directly rather than through a
    /// command handler, so they mint a permit without a dry-run flag to
    /// consult. Every non-test caller must go through
    /// `ApplyPermit::granted_unless_dry_run`.
    fn permit() -> callisto_model::ApplyPermit {
        callisto_model::ApplyPermit::force_for_tests()
    }
    use super::*;
    use callisto_model::{ManifestDecl, ManifestFormat, ManifestRole};
    use tempfile::tempdir;

    #[test]
    fn persist_is_exposed_as_a_manifest_trait_method() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        let content = "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n";
        fs::write(&manifest_path, content).unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
        <CargoToml as Manifest>::persist(&mut manifest, &permit()).unwrap();

        let after = fs::read_to_string(&manifest_path).unwrap();
        assert_eq!(
            after, content,
            "persist() with no prior mutation must reproduce the file unchanged"
        );
    }

    #[test]
    fn parses_cargo_toml_and_updates_version() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        let content = r#"[package]
name = "my-crate"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = "1.0"
"#;
        fs::write(&manifest_path, content).unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
        assert_eq!(manifest.package_name().unwrap(), "my-crate");
        assert_eq!(manifest.current_version().unwrap().render(), "0.1.0");

        let new_ver = Version::parse("0.2.0", VersionGrammar::SemVer).unwrap();
        manifest.write_version(&new_ver, &permit()).unwrap();
        manifest.persist(&permit()).unwrap();

        let updated_content = fs::read_to_string(&manifest_path).unwrap();
        assert!(updated_content.contains("version = \"0.2.0\""));
    }

    #[test]
    fn write_version_does_not_touch_disk_until_persist_called() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        let content = "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n";
        fs::write(&manifest_path, content).unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
        let new_ver = Version::parse("0.2.0", VersionGrammar::SemVer).unwrap();
        manifest.write_version(&new_ver, &permit()).unwrap();

        let unchanged = fs::read_to_string(&manifest_path).unwrap();
        assert_eq!(
            unchanged, content,
            "write_version alone must not write to disk"
        );

        manifest.persist(&permit()).unwrap();
        let updated = fs::read_to_string(&manifest_path).unwrap();
        assert!(updated.contains("version = \"0.2.0\""));
    }

    #[test]
    fn handles_utf8_bom_cargo_toml() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        fs::write(
            &manifest_path,
            callisto_fixtures::corpus::cargo_toml_bom_sample(),
        )
        .unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let manifest = CargoToml::open(&decl, &ctx).unwrap();
        assert_eq!(manifest.package_name().unwrap(), "bom-crate");
        assert_eq!(manifest.current_version().unwrap().render(), "1.0.0");
    }

    #[test]
    fn preserves_bom_on_write_round_trip() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        fs::write(
            &manifest_path,
            callisto_fixtures::corpus::cargo_toml_bom_sample(),
        )
        .unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
        let new_ver = Version::parse("1.0.1", VersionGrammar::SemVer).unwrap();
        manifest.write_version(&new_ver, &permit()).unwrap();
        manifest.persist(&permit()).unwrap();

        let updated_bytes = fs::read(&manifest_path).unwrap();
        let updated = String::from_utf8(updated_bytes).unwrap();

        assert!(
            updated.starts_with('\u{FEFF}'),
            "expected UTF-8 BOM to survive write, got:\n{updated:?}"
        );
        assert!(updated.contains("version = \"1.0.1\""));
    }

    #[test]
    fn empty_cargo_toml_returns_parse_error_not_panic() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        fs::write(&manifest_path, "").unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        // An empty TOML document is technically valid (empty table); must not panic.
        let manifest = CargoToml::open(&decl, &ctx).unwrap();
        assert!(manifest.package_name().is_err());
    }

    #[test]
    fn bom_only_cargo_toml_returns_parse_error_not_panic() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        fs::write(&manifest_path, "\u{FEFF}").unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        // BOM-only content strips down to an empty string, which parses as an
        // empty (valid) TOML document; must not panic.
        let manifest = CargoToml::open(&decl, &ctx).unwrap();
        assert!(manifest.package_name().is_err());
    }

    #[test]
    fn garbage_cargo_toml_returns_parse_error_not_panic() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        fs::write(&manifest_path, "\u{FEFF}not valid toml {{{").unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let result = CargoToml::open(&decl, &ctx);
        assert!(matches!(result, Err(ManifestError::Parse { .. })));
    }

    #[test]
    fn write_dependency_preserves_trailing_comment_on_full_table_version() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        let content = r#"[workspace]
members = ["crates/*"]

[workspace.dependencies.serde]
version = "1.0.0" # pinned intentionally, do not bump lightly
path = "../serde"
"#;
        fs::write(&manifest_path, content).unwrap();

        let mut resolver = WorkspaceCargoResolver::load(&manifest_path).unwrap();
        let new_spec = DepSpec::Range(
            VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
            "^1.1.0".to_string(),
        );
        resolver
            .write_dependency("serde", new_spec, &permit())
            .unwrap();

        let updated = fs::read_to_string(&manifest_path).unwrap();
        assert!(updated.contains("version = \"^1.1.0\""));
        assert!(
            updated.contains("# pinned intentionally, do not bump lightly"),
            "expected trailing comment on version field to survive full-table write, got:\n{updated}"
        );
    }

    #[test]
    fn write_dependency_preserves_decor_on_inline_table_version() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        // Custom spacing decor around the `version` key/value inside the inline table.
        // TOML forbids `#` comments inside inline tables, so the fidelity-loss proxy
        // here is the leading/trailing whitespace decor attached to the version value,
        // which write_dependency must preserve just like it does for the plain-string
        // and full-table branches.
        let content = "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.dependencies]\nserde = { version =   \"1.0.0\"  , path = \"../serde\" }\n";
        fs::write(&manifest_path, content).unwrap();

        let mut resolver = WorkspaceCargoResolver::load(&manifest_path).unwrap();
        let new_spec = DepSpec::Range(
            VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
            "^1.1.0".to_string(),
        );
        resolver
            .write_dependency("serde", new_spec, &permit())
            .unwrap();

        let updated = fs::read_to_string(&manifest_path).unwrap();
        assert!(updated.contains("^1.1.0"));
        assert!(
            updated.contains("version =   \"^1.1.0\"  ,"),
            "expected custom decor around version field to survive inline-table write, got:\n{updated}"
        );
    }

    #[test]
    fn preserves_tab_indentation_cargo_toml() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        fs::write(
            &manifest_path,
            callisto_fixtures::corpus::cargo_toml_tab_indented_sample(),
        )
        .unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
        assert_eq!(manifest.package_name().unwrap(), "tabbed-crate");

        let new_ver = Version::parse("1.0.1", VersionGrammar::SemVer).unwrap();
        manifest.write_version(&new_ver, &permit()).unwrap();
        manifest.persist(&permit()).unwrap();

        let updated = fs::read_to_string(&manifest_path).unwrap();
        assert!(updated.contains("\tversion = \"1.0.1\""));
        assert!(updated.contains("\tname = \"tabbed-crate\""));
    }

    #[test]
    fn test_cargo_is_publishable() {
        let dir = tempdir().unwrap();
        let pub_false_path = dir.path().join("Cargo.toml");
        fs::write(
            &pub_false_path,
            "[package]\nname = \"dummy\"\nversion = \"0.1.0\"\npublish = false\n",
        )
        .unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let manifest = CargoToml::open(&decl, &ctx).unwrap();
        assert!(!manifest.is_publishable());
    }

    #[test]
    fn round_trip_cargo_spec() {
        let req = VersionReq::parse("^1.0.0", Ecosystem::Cargo).unwrap();
        let spec = DepSpec::Range(req, "^1.0.0".to_string());
        let target = Version::parse("1.2.0", VersionGrammar::SemVer).unwrap();

        let updated = round_trip(&spec, &target).unwrap();
        if let DepSpec::Range(_, s) = updated {
            assert_eq!(s, "^1.2.0");
        } else {
            panic!("expected Range DepSpec");
        }
    }

    #[test]
    fn test_cargo_inline_table_dependency_update() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        let content = r#"[package]
name = "my-crate"
version = "0.1.0"
edition = "2021"

[dependencies]
helper = { version = "1.0.0", path = "../helper" }
"#;
        fs::write(&manifest_path, content).unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
        let new_spec = DepSpec::Range(
            VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
            "^1.1.0".to_string(),
        );
        manifest
            .update_dependency_spec(
                "helper",
                callisto_model::DepKind::Runtime,
                new_spec,
                &permit(),
            )
            .unwrap();

        let updated = fs::read_to_string(&manifest_path).unwrap();
        assert!(updated.contains("version = \"^1.1.0\""));
        assert!(updated.contains("path = \"../helper\""));
    }

    #[test]
    fn test_cargo_decor_comment_preservation() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        let content = r#"[package]
name = "my-crate"
version = "0.1.0" # preserve this inline comment
edition = "2021"
"#;
        fs::write(&manifest_path, content).unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
        let new_ver = Version::parse("0.2.0", VersionGrammar::SemVer).unwrap();
        manifest.write_version(&new_ver, &permit()).unwrap();
        manifest.persist(&permit()).unwrap();

        let updated = fs::read_to_string(&manifest_path).unwrap();
        assert!(updated.contains("version = \"0.2.0\" # preserve this inline comment"));
    }

    #[test]
    fn update_dependency_spec_preserves_decor_on_full_table_version() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        let content = r#"[package]
name = "my-crate"
version = "0.1.0"

[dependencies.helper]
version = "1.0.0" # pinned intentionally, do not bump lightly
path = "../helper"
"#;
        fs::write(&manifest_path, content).unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
        let new_spec = DepSpec::Range(
            VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
            "^1.1.0".to_string(),
        );
        manifest
            .update_dependency_spec(
                "helper",
                callisto_model::DepKind::Runtime,
                new_spec,
                &permit(),
            )
            .unwrap();

        let updated = fs::read_to_string(&manifest_path).unwrap();
        assert!(updated.contains("version = \"^1.1.0\""));
        assert!(
            updated.contains("# pinned intentionally, do not bump lightly"),
            "expected trailing comment on version field to survive full-table write, got:\n{updated}"
        );
    }

    #[test]
    fn test_cargo_dependency_version_update_decor_preservation() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        let content = r#"[package]
name = "my-crate"
version = "0.1.0"

[dependencies]
helper = { version = "1.0.0", path = "../helper" } # dep comment
"#;
        fs::write(&manifest_path, content).unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
        let new_spec = DepSpec::Range(
            VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
            "^1.1.0".to_string(),
        );
        manifest
            .update_dependency_spec(
                "helper",
                callisto_model::DepKind::Runtime,
                new_spec,
                &permit(),
            )
            .unwrap();

        let updated = fs::read_to_string(&manifest_path).unwrap();
        assert!(updated.contains("# dep comment"));
    }

    /// Regression test: applying a version bump to a workspace-inheriting member must
    /// write an explicit pinned version on the member's [package] section without
    /// touching [workspace.package] in the root Cargo.toml.
    #[test]
    fn write_version_pins_explicitly_on_workspace_inheriting_member() {
        use std::sync::Arc;

        let dir = tempdir().unwrap();

        // Workspace root: version "1.0.0" in [workspace.package]
        let root_cargo_path = dir.path().join("Cargo.toml");
        fs::write(
            &root_cargo_path,
            r#"[workspace]
members = ["member"]

[workspace.package]
version = "1.0.0"
"#,
        )
        .unwrap();

        // Member with version.workspace = true
        let member_dir = dir.path().join("member");
        fs::create_dir_all(&member_dir).unwrap();
        let member_cargo_path = member_dir.join("Cargo.toml");
        fs::write(
            &member_cargo_path,
            r#"[package]
name = "member-crate"
version.workspace = true
edition = "2021"
"#,
        )
        .unwrap();

        // Build workspace inheritance context
        let ws_resolver = WorkspaceCargoResolver::load(&root_cargo_path).unwrap();
        let inheritance = Arc::new(ws_resolver.inheritance().unwrap());

        let decl = ManifestDecl::new(
            "member/Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: Some(inheritance),
            npm_workspace_kind: None,
        };

        let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
        // Precondition: version is inherited as 1.0.0
        assert_eq!(manifest.current_version().unwrap().render(), "1.0.0");

        // Apply bump to 1.1.0 on the MEMBER ONLY
        let new_ver = Version::parse("1.1.0", VersionGrammar::SemVer).unwrap();
        manifest.write_version(&new_ver, &permit()).unwrap();
        manifest.persist(&permit()).unwrap();

        // Workspace root must be UNCHANGED
        let root_updated = fs::read_to_string(&root_cargo_path).unwrap();
        assert!(
            root_updated.contains("version = \"1.0.0\""),
            "workspace root version must remain 1.0.0, got:\n{root_updated}"
        );
        assert!(
            !root_updated.contains("1.1.0"),
            "workspace root must NOT contain 1.1.0, got:\n{root_updated}"
        );

        // Member must have an explicit pinned version, not workspace = true
        let member_updated = fs::read_to_string(&member_cargo_path).unwrap();
        assert!(
            member_updated.contains("version = \"1.1.0\""),
            "member must have pinned version 1.1.0, got:\n{member_updated}"
        );
        assert!(
            !member_updated.contains("workspace = true"),
            "member must not retain workspace inheritance after pinning, got:\n{member_updated}"
        );
    }

    /// Spec: a "virtual workspace" root `Cargo.toml` (one that defines only
    /// `[workspace]` and has no `[package]` section) must make `package_name()`
    /// return `Err(ManifestError::MissingField)`. Virtual workspaces are common
    /// in Cargo monorepos where the root manifest coordinates workspace members
    /// but is not itself a publishable crate.
    #[test]
    fn virtual_workspace_cargo_toml_has_no_package_name() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        let content = r#"[workspace]
members = ["crates/*"]

[workspace.package]
version = "0.1.0"
edition = "2021"

[workspace.dependencies]
serde = { version = "1.0", features = ["derive"] }
"#;
        fs::write(&manifest_path, content).unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        // A virtual workspace root has no [package] table: open must succeed
        // (the file is valid TOML), but package_name must return an error.
        let manifest = CargoToml::open(&decl, &ctx).unwrap();
        let result = manifest.package_name();
        assert!(
            matches!(
                result,
                Err(ManifestError::MissingField {
                    field: "package.name",
                    ..
                })
            ),
            "virtual workspace must return MissingField for package.name, got: {result:?}"
        );
    }

    /// Spec: a crate manifest that uses `version.workspace = true` must be
    /// openable when a `WorkspaceCargoResolver` context is supplied.  Without
    /// that context the open must fail with a descriptive `Read` error (not a
    /// panic), because `workspace = true` without the resolver cannot be
    /// resolved and silently falling through would produce wrong results.
    #[test]
    fn cargo_toml_with_workspace_version_requires_resolver_context() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        let content = r#"[package]
name = "my-inherited-crate"
version.workspace = true
edition = "2021"
"#;
        fs::write(&manifest_path, content).unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();

        // Without a workspace resolver, open must return Err — not panic.
        let ctx_no_ws = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };
        let result = CargoToml::open(&decl, &ctx_no_ws);
        assert!(
            result.is_err(),
            "opening version.workspace = true without resolver must return Err"
        );
        let err_msg = result.err().unwrap().to_string();
        assert!(
            err_msg.contains("workspace"),
            "error message must mention 'workspace', got: {err_msg}"
        );

        // With a resolver that provides the workspace version, open must succeed
        // and current_version must return the workspace-inherited version.
        let root_cargo_path = dir.path().join("root_Cargo.toml");
        let root_content = r#"[workspace]
members = ["."]

[workspace.package]
version = "4.5.6"
"#;
        fs::write(&root_cargo_path, root_content).unwrap();

        let inheritance = WorkspaceCargoResolver::load(&root_cargo_path)
            .unwrap()
            .inheritance()
            .unwrap();

        let ctx_with_ws = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: Some(std::sync::Arc::new(inheritance)),
            npm_workspace_kind: None,
        };
        let manifest = CargoToml::open(&decl, &ctx_with_ws).unwrap();
        assert_eq!(
            manifest.package_name().unwrap(),
            "my-inherited-crate",
            "package name must be read from [package].name even with workspace version"
        );
        assert_eq!(
            manifest.current_version().unwrap().render(),
            "4.5.6",
            "current_version must be inherited from the workspace resolver"
        );
    }

    /// A compound Cargo range `>=1.0.0, <2.0.0` whose lower bound is updated
    /// past the preserved upper bound (e.g., target `2.1.0`) must return `None`
    /// rather than silently producing the impossible `>=2.1.0, <2.0.0`.
    #[test]
    fn round_trip_compound_range_crossing_upper_bound_returns_none() {
        let req = VersionReq::parse(">=1.0.0, <2.0.0", Ecosystem::Cargo).unwrap();
        let spec = DepSpec::Range(req, ">=1.0.0, <2.0.0".to_string());
        let target = Version::parse("2.1.0", VersionGrammar::SemVer).unwrap();

        let result = round_trip(&spec, &target);
        assert!(
            result.is_none(),
            "compound range crossing upper bound must return None, got: {result:?}"
        );
    }

    /// A compound Cargo range within the same major version is safe to rewrite.
    #[test]
    fn round_trip_compound_range_within_upper_bound_is_rewritten() {
        let req = VersionReq::parse(">=1.0.0, <2.0.0", Ecosystem::Cargo).unwrap();
        let spec = DepSpec::Range(req, ">=1.0.0, <2.0.0".to_string());
        let target = Version::parse("1.5.0", VersionGrammar::SemVer).unwrap();

        let result = round_trip(&spec, &target);
        match result {
            Some(DepSpec::Range(_, rendered)) => {
                assert_eq!(
                    rendered, ">=1.5.0, <2.0.0",
                    "lower clause must be rewritten to the target while the upper bound clause is preserved unchanged"
                );
            }
            other => panic!(
                "compound range within upper bound must be rewritten to Some(..), got: {other:?}"
            ),
        }
    }

    use proptest::prelude::*;
    proptest! {
        #[test]
        fn proptest_cargo_toml_parse_never_panics(s in "\\PC*") {
            let _res = s.parse::<toml_edit::DocumentMut>();
        }
    }

    #[test]
    fn update_dependency_spec_self_referential_workspace_delegation_returns_invariant_violation() {
        let dir = tempdir().unwrap();
        let root_cargo_path = dir.path().join("Cargo.toml");
        let content = "[workspace]\nmembers = [\".\"]\n\n[package]\nname = \"self-referential-root\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = { workspace = true }\n\n[workspace.dependencies]\nserde = \"1.0.0\"\n";
        fs::write(&root_cargo_path, content).unwrap();

        let inheritance = WorkspaceCargoResolver::load(&root_cargo_path)
            .unwrap()
            .inheritance()
            .unwrap();

        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: Some(Arc::new(inheritance)),
            npm_workspace_kind: None,
        };

        let mut manifest = CargoToml::open(&decl, &ctx).unwrap();

        let new_spec = DepSpec::Range(
            VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
            "^1.1.0".to_string(),
        );
        let result = manifest.update_dependency_spec(
            "serde",
            callisto_model::DepKind::Runtime,
            new_spec,
            &permit(),
        );

        assert!(
            matches!(result, Err(ManifestError::InvariantViolation { .. })),
            "expected InvariantViolation, got: {result:?}"
        );

        let on_disk = fs::read_to_string(&root_cargo_path).unwrap();
        assert_eq!(on_disk, content, "manifest file must be unchanged on disk");

        manifest.persist(&permit()).unwrap();
        let after_persist = fs::read_to_string(&root_cargo_path).unwrap();
        assert_eq!(
            after_persist, content,
            "in-memory document must not have been mutated by the failed call"
        );
    }
}
