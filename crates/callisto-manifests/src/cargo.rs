use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use callisto_model::{
    workspace_relative, ApplyPermit, DepKind, DepSpec, DependencyEntry, Ecosystem, ManifestDecl, ManifestError,
    ManifestFormat, ManifestRole, Version, VersionGrammar, VersionReq,
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
    fingerprint: crate::common::FormatFingerprint,
}

impl CargoToml {
    pub fn open(decl: &ManifestDecl, ctx: &OpenContext<'_>) -> Result<Self, ManifestError> {
        let rel_path = decl.path.clone();
        let abs_path = ctx.workspace_root.join(&rel_path);

        let content = fs::read_to_string(&abs_path).map_err(|e| ManifestError::Read {
            path: rel_path.clone(),
            message: e.to_string(),
        })?;

        let fingerprint = crate::common::FormatFingerprint::detect(&content);
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
                message: "declares .workspace = true but no WorkspaceCargoResolver context was supplied".to_string(),
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
            fingerprint,
        })
    }
}

/// Extracts `[package].name` from an already-parsed `Cargo.toml` document.
/// Pure and I/O-free -- operates on a document the caller already has,
/// whether that's a fully-opened [`CargoToml`] (via [`Manifest::package_name`])
/// or a bare parse a caller made for some other purpose entirely (e.g.
/// [`crate::read_identity`], which needs only a package name and deliberately
/// does not go through the full [`CargoToml::open`] lifecycle).
pub fn cargo_package_name(doc: &toml_edit::DocumentMut) -> Option<&str> {
    doc.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str())
}

/// Rewrites a dependency entry's `version` to `new_str`, preserving decor
/// across the three shapes Cargo allows for a dependency value: a bare
/// version string, an inline table with a `version` key, or a full
/// `[dependencies.name]` table with a `version` key. `Err(())` for any
/// other shape -- callers map that to their own
/// `ManifestError::UnrecognizedDependencyValue` (which needs a `path` this
/// function has no access to). Shared by the member-manifest
/// ([`CargoToml::update_dependency_spec`]) and workspace-root
/// ([`WorkspaceCargoResolver::write_dependency`]) dependency-write paths,
/// which were previously two copies of this exact three-shape dance,
/// differing only in which path they report on error.
fn set_dependency_version(item: &mut toml_edit::Item, new_str: String) -> Result<(), ()> {
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
        } else {
            return Err(());
        }
    } else if let Some(tbl) = item.as_table_mut() {
        let decor = tbl.get("version").and_then(|i| i.as_value()).map(|v| v.decor().clone());
        tbl.insert("version", toml_edit::value(new_str));
        if let Some(decor) = decor {
            if let Some(new_item) = tbl.get_mut("version").and_then(|i| i.as_value_mut()) {
                *new_item.decor_mut() = decor;
            }
        }
    } else {
        return Err(());
    }
    Ok(())
}

/// [`crate::read_identity`]'s `Cargo.toml` implementation: parses `source`
/// and extracts `[package].name` plus a [`crate::VersionSource`] for
/// `[package].version` -- `InheritedFromWorkspace` when it's
/// `version.workspace = true`, `Literal` when it's a bare string, `None`
/// when the field or the whole `[package]` table is absent.
pub(crate) fn identity_from_source(source: &str, path: &Path) -> Result<crate::ManifestIdentity, ManifestError> {
    let clean = source.strip_prefix('\u{FEFF}').unwrap_or(source);
    let doc: toml_edit::DocumentMut = clean.parse().map_err(|e: toml_edit::TomlError| ManifestError::Parse {
        path: path.to_path_buf(),
        format: ManifestFormat::CargoToml,
        message: e.to_string(),
    })?;

    let name = cargo_package_name(&doc).map(str::to_string);
    let version = doc.get("package").and_then(|p| p.get("version")).and_then(|v| {
        if let Some(s) = v.as_str() {
            Some(crate::VersionSource::Literal(s.to_string()))
        } else if v.get("workspace").and_then(|w| w.as_bool()) == Some(true) {
            Some(crate::VersionSource::InheritedFromWorkspace)
        } else {
            None
        }
    });

    Ok(crate::ManifestIdentity { name, version })
}

pub(crate) fn workspace_version_from_source(source: &str, path: &Path) -> Result<Option<Version>, ManifestError> {
    let clean = source.strip_prefix('\u{FEFF}').unwrap_or(source);
    let doc: toml_edit::DocumentMut = clean.parse().map_err(|e: toml_edit::TomlError| ManifestError::Parse {
        path: path.to_path_buf(),
        format: ManifestFormat::CargoToml,
        message: e.to_string(),
    })?;
    workspace_package_version(&doc, path)
}

fn workspace_package_version(doc: &toml_edit::DocumentMut, path: &Path) -> Result<Option<Version>, ManifestError> {
    let Some(raw) = doc
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
    else {
        return Ok(None);
    };
    Version::parse(raw, VersionGrammar::SemVer)
        .map(Some)
        .map_err(|e| ManifestError::Parse {
            path: path.to_path_buf(),
            format: ManifestFormat::CargoToml,
            message: format!("invalid [workspace.package] version: {e}"),
        })
}

impl Manifest for CargoToml {
    fn persist(&mut self, permit: &ApplyPermit) -> Result<(), ManifestError> {
        let text = self.fingerprint.apply(&self.document.to_string());
        crate::atomic::atomic_write(&self.absolute, &text, permit).map_err(|e| ManifestError::Write {
            path: self.path.clone(),
            message: e.to_string(),
        })?;
        crate::record_persist_call();
        Ok(())
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

    fn role(&self) -> ManifestRole {
        self.role.clone()
    }

    fn bin_names(&self) -> Vec<String> {
        let Some(package) = cargo_package_name(&self.document) else {
            return Vec::new();
        };
        let mut names = BTreeSet::new();
        if let Some(bins) = self.document.get("bin").and_then(|bin| bin.as_array_of_tables()) {
            for target in bins.iter() {
                let name = target
                    .get("name")
                    .and_then(|name| name.as_str())
                    .map(str::to_owned)
                    .or_else(|| {
                        let path = target.get("path").and_then(|path| path.as_str()).map(Path::new)?;
                        let stem = path.file_stem()?;
                        if stem == "main" {
                            path.parent()?.file_name()?.to_str().map(str::to_owned)
                        } else {
                            stem.to_str().map(str::to_owned)
                        }
                    })
                    .unwrap_or_else(|| package.to_owned());
                names.insert(name);
            }
        }
        let autobins = self
            .document
            .get("package")
            .and_then(|item| item.get("autobins"))
            .and_then(|item| item.as_bool())
            .unwrap_or(true);
        if autobins {
            if self
                .absolute
                .parent()
                .is_some_and(|dir| dir.join("src/main.rs").is_file())
            {
                names.insert(package.to_owned());
            }
            if let Some(bin_dir) = self.absolute.parent().map(|dir| dir.join("src/bin")) {
                if let Ok(entries) = fs::read_dir(&bin_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
                            if let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) {
                                names.insert(name.to_owned());
                            }
                        } else if path.is_dir() && path.join("main.rs").is_file() {
                            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                                names.insert(name.to_owned());
                            }
                        }
                    }
                }
            }
        }
        names.into_iter().collect()
    }

    fn napi_lib_name(&self) -> Option<String> {
        let lib = self.document.get("lib");
        let cdylib = lib
            .and_then(|lib| lib.get("crate-type"))
            .and_then(|types| types.as_array())
            .is_some_and(|types| types.iter().any(|kind| kind.as_str() == Some("cdylib")));
        let napi = self
            .document
            .get("dependencies")
            .and_then(|deps| deps.as_table_like())
            .is_some_and(|deps| deps.contains_key("napi"));
        if !(cdylib && napi) {
            return None;
        }
        lib.and_then(|lib| lib.get("name"))
            .and_then(|name| name.as_str())
            .or_else(|| cargo_package_name(&self.document))
            .map(|name| name.replace('-', "_"))
    }

    fn package_name(&self) -> Result<String, ManifestError> {
        let name = cargo_package_name(&self.document).ok_or_else(|| ManifestError::MissingField {
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

        Version::parse(raw, VersionGrammar::SemVer).map_err(|source| ManifestError::InvalidVersion {
            path: self.path.clone(),
            raw: raw.to_string(),
            source,
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

        crate::common::set_scalar_preserving_decor(pkg, "version", v.render());
        Ok(())
    }

    fn iter_dependencies(&self) -> Box<dyn Iterator<Item = DependencyEntry> + '_> {
        let mut entries = Vec::new();

        for (section_name, section_kind) in [
            ("dependencies", DepKind::Runtime),
            ("dev-dependencies", DepKind::Dev),
            ("build-dependencies", DepKind::Build),
        ] {
            if let Some(table) = self.document.get(section_name).and_then(|t| t.as_table_like()) {
                for (name, item) in table.iter() {
                    let is_inherited = self.inherited_deps.contains(&(section_kind, name.to_string()));
                    let mut kind = section_kind;
                    let local_spec = parse_cargo_dep_item(item, &mut kind);
                    let inherited_dep = self.inheritance.as_ref().and_then(|inh| inh.inherited(name));
                    let package_name = if is_inherited {
                        inherited_dep.map(|dep| dep.package_name).unwrap_or(name)
                    } else {
                        cargo_dependency_package_name(name, item)
                    };

                    let spec = if is_inherited {
                        if let Some(inherited_dep) = inherited_dep {
                            inherited_dep.spec.clone()
                        } else {
                            DepSpec::Opaque("workspace = true (unresolved)".to_string())
                        }
                    } else {
                        local_spec
                    };

                    entries.push(DependencyEntry {
                        name: name.to_string(),
                        package_name: package_name.to_string(),
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

        let section_kind = if kind == DepKind::Optional {
            DepKind::Runtime
        } else {
            kind
        };
        if self.inherited_deps.contains(&(section_kind, name.to_string())) {
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
            ws_res.write_dependency(name, new, permit)?;
            return ws_res.persist(permit);
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

        let item = table.get_mut(name).ok_or_else(|| ManifestError::DependencyNotFound {
            path: self.path.clone(),
            name: name.to_string(),
            kind,
        })?;

        let new_str = new.render();

        set_dependency_version(item, new_str).map_err(|()| ManifestError::UnrecognizedDependencyValue {
            path: self.path.clone(),
            name: name.to_string(),
        })?;

        Ok(())
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

fn cargo_dependency_package_name<'a>(name: &'a str, item: &'a toml_edit::Item) -> &'a str {
    item.get("package").and_then(|package| package.as_str()).unwrap_or(name)
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
    pub dependencies: BTreeMap<String, DependencyEntry>,
}

impl WorkspaceInheritance {
    pub fn inherited(&self, name: &str) -> Option<InheritedDep<'_>> {
        let entry = self.dependencies.get(name)?;
        Some(InheritedDep {
            spec: &entry.spec,
            package_name: &entry.package_name,
            declared_in: &self.root_manifest,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InheritedDep<'a> {
    pub spec: &'a DepSpec,
    pub package_name: &'a str,
    pub declared_in: &'a Path,
}

pub struct WorkspaceCargoResolver {
    root_path: PathBuf,
    absolute_path: PathBuf,
    document: toml_edit::DocumentMut,
}

impl WorkspaceCargoResolver {
    pub fn load(root_manifest_path: &Path) -> Result<Self, ManifestError> {
        crate::record_resolver_load_call();
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

        let doc: toml_edit::DocumentMut = content
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
        let version = workspace_package_version(&self.document, &self.root_path)
            .ok()
            .flatten();

        let mut dependencies = BTreeMap::new();
        if let Some(table) = self
            .document
            .get("workspace")
            .and_then(|w| w.get("dependencies"))
            .and_then(|d| d.as_table_like())
        {
            for (name, item) in table.iter() {
                let mut kind = DepKind::Runtime;
                let spec = parse_cargo_dep_item(item, &mut kind);
                dependencies.insert(
                    name.to_string(),
                    DependencyEntry {
                        name: name.to_string(),
                        package_name: cargo_dependency_package_name(name, item).to_string(),
                        kind,
                        spec,
                        inherited: false,
                    },
                );
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
        workspace_package_version(&self.document, &self.root_path)
    }

    /// Pure in-memory mutation of `[workspace.package] version` -- no disk
    /// I/O. Mirrors `CargoToml`/`PackageJson`/`PyprojectToml`'s
    /// `write_version`, which likewise never touches disk; callers must
    /// call [`Self::persist`] explicitly afterward. `_permit` is accepted
    /// (unused) purely for call-site symmetry with `persist`, which does
    /// require one.
    pub fn write_version(&mut self, v: &Version, _permit: &ApplyPermit) -> Result<(), ManifestError> {
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

        crate::common::set_scalar_preserving_decor(pkg, "version", v.render());
        Ok(())
    }

    /// Pure in-memory mutation of a `[workspace.dependencies]` entry -- no
    /// disk I/O. See [`Self::write_version`]'s doc for why this no longer
    /// persists internally; callers must call [`Self::persist`] explicitly
    /// afterward.
    pub fn write_dependency(&mut self, name: &str, new: DepSpec, _permit: &ApplyPermit) -> Result<(), ManifestError> {
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

        let item = deps.get_mut(name).ok_or_else(|| ManifestError::DependencyNotFound {
            path: self.root_path.clone(),
            name: name.to_string(),
            kind: DepKind::Runtime,
        })?;

        let new_str = new.render();
        set_dependency_version(item, new_str).map_err(|()| ManifestError::UnrecognizedDependencyValue {
            path: self.root_path.clone(),
            name: name.to_string(),
        })?;

        Ok(())
    }

    /// Flushes the in-memory document to disk via
    /// [`crate::atomic::atomic_write`]. Distinct, explicitly-called step --
    /// see [`Self::write_version`]/[`Self::write_dependency`]'s docs --
    /// mirroring the `Manifest` trait's `persist` across `CargoToml`,
    /// `PackageJson`, and `PyprojectToml`.
    pub fn persist(&mut self, permit: &ApplyPermit) -> Result<(), ManifestError> {
        let text = self.document.to_string();
        crate::atomic::atomic_write(&self.absolute_path, &text, permit).map_err(|e| ManifestError::Write {
            path: self.root_path.clone(),
            message: e.to_string(),
        })?;
        crate::record_resolver_persist_call();
        Ok(())
    }
}

#[cfg(test)]
mod tests;
