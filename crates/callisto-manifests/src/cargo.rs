use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use callisto_model::{
    workspace_relative, ApplyPermit, DepKind, DepLocator, DepSpec, DependencyEntry, Ecosystem, ManifestDecl,
    ManifestError, ManifestFormat, ManifestRole, Version, VersionGrammar, VersionReq,
};

use crate::{Manifest, OpenContext};

pub struct CargoToml {
    path: PathBuf,
    absolute: PathBuf,
    workspace_root: PathBuf,
    role: ManifestRole,
    document: toml_edit::DocumentMut,
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

        let inherits_dependencies = dependency_locations(&doc).into_iter().any(|(section, _, target)| {
            dependency_table(&doc, section, target.as_deref())
                .is_some_and(|table| table.iter().any(|(_, item)| is_inherited_item(item)))
        });

        let inheritance = if inherited_version || inherits_dependencies {
            Some(ctx.cargo_inheritance(&rel_path, &doc)?)
        } else {
            None
        };

        Ok(CargoToml {
            path: rel_path,
            absolute: abs_path,
            workspace_root: ctx.workspace_root.to_path_buf(),
            role: decl.role.clone(),
            document: doc,
            inherited_version,
            inheritance,
            fingerprint,
        })
    }
}

/// What has been learned about workspace manifests while reading many packages, so each one is parsed once however many
/// packages ask about it.
#[derive(Debug, Default)]
pub struct WorkspaceReads {
    declares: HashMap<PathBuf, bool>,
    versions: HashMap<PathBuf, Option<Version>>,
}

impl WorkspaceReads {
    /// The workspace-relative `Cargo.toml` of the Cargo workspace that governs the package at `manifest`, found the way
    /// Cargo finds it: the directory named by `package.workspace`, else the nearest manifest at or above the package's
    /// directory that has a `workspace` table. The search stops at the workspace root. `doc` is the package's own
    /// manifest and `manifest_at` supplies any other workspace-relative manifest's text, or `None` when there is none,
    /// so a past commit can answer as well as the working tree.
    ///
    /// # Errors
    ///
    /// A manifest on the way up that cannot be read or parsed (skipping it could settle on a different root), or a
    /// `package.workspace` that leaves the workspace root or names a manifest with no `workspace` table.
    pub fn enclosing<E: From<ManifestError>>(
        &mut self,
        manifest: &Path,
        doc: &toml_edit::DocumentMut,
        mut manifest_at: impl FnMut(&Path) -> Result<Option<String>, E>,
    ) -> Result<Option<PathBuf>, E> {
        let dir = manifest.parent().unwrap_or(Path::new(""));
        let read_error = |path: &Path, message: String| ManifestError::Read {
            path: path.to_path_buf(),
            message,
        };

        if let Some(named) = doc
            .get("package")
            .and_then(|package| package.get("workspace"))
            .and_then(toml_edit::Item::as_str)
        {
            let root_manifest = workspace_relative(dir.join(named))
                .map_err(|error| {
                    read_error(
                        manifest,
                        format!("`package.workspace = \"{named}\"` is unusable: {error}"),
                    )
                })?
                .join("Cargo.toml");
            return if self.declares(&root_manifest, &mut manifest_at)? {
                Ok(Some(root_manifest))
            } else {
                Err(read_error(
                    manifest,
                    format!(
                        "`package.workspace = \"{named}\"` names `{}`, which declares no `[workspace]`",
                        root_manifest.display()
                    ),
                )
                .into())
            };
        }

        for candidate in dir.ancestors() {
            let candidate_manifest = candidate.join("Cargo.toml");
            let declares_workspace = if candidate == dir {
                declares_workspace(doc)
            } else {
                self.declares(&candidate_manifest, &mut manifest_at)?
            };
            if declares_workspace {
                return Ok(Some(candidate_manifest));
            }
        }
        Ok(None)
    }

    /// The `[workspace.package]` version of the workspace manifest `root_manifest`, or `None` when it declares none.
    ///
    /// # Errors
    ///
    /// `root_manifest` is missing, cannot be parsed, or declares an invalid version; or `manifest_at` fails.
    pub fn version<E: From<ManifestError>>(
        &mut self,
        root_manifest: &Path,
        mut manifest_at: impl FnMut(&Path) -> Result<Option<String>, E>,
    ) -> Result<Option<Version>, E> {
        if let Some(known) = self.versions.get(root_manifest) {
            return Ok(known.clone());
        }
        let text = manifest_at(root_manifest)?.ok_or_else(|| ManifestError::Read {
            path: root_manifest.to_path_buf(),
            message: "No such file or directory".to_string(),
        })?;
        let version = workspace_version_from_source(&text, root_manifest)?;
        self.versions.insert(root_manifest.to_path_buf(), version.clone());
        Ok(version)
    }

    fn declares<E: From<ManifestError>>(
        &mut self,
        manifest: &Path,
        manifest_at: &mut impl FnMut(&Path) -> Result<Option<String>, E>,
    ) -> Result<bool, E> {
        if let Some(known) = self.declares.get(manifest) {
            return Ok(*known);
        }
        let declared = match manifest_at(manifest)? {
            Some(text) => declares_workspace(&parse_manifest(manifest, &text)?),
            None => false,
        };
        self.declares.insert(manifest.to_path_buf(), declared);
        Ok(declared)
    }
}

/// Reads workspace-relative manifest text from the working tree under `workspace_root`; `None` when there is none.
pub fn read_manifest_text(workspace_root: &Path) -> impl FnMut(&Path) -> Result<Option<String>, ManifestError> + '_ {
    move |manifest| {
        callisto_model::strict_fs::read_optional(&workspace_root.join(manifest)).map_err(|error| ManifestError::Read {
            path: manifest.to_path_buf(),
            message: error.to_string(),
        })
    }
}

/// The workspace-relative `Cargo.toml` of the Cargo workspace that governs the package at `manifest`, read from the
/// working tree; see [`WorkspaceReads::enclosing`]. `doc` is the package's own manifest.
pub fn enclosing_workspace_manifest(
    workspace_root: &Path,
    manifest: &Path,
    doc: &toml_edit::DocumentMut,
) -> Result<Option<PathBuf>, ManifestError> {
    WorkspaceReads::default().enclosing(manifest, doc, read_manifest_text(workspace_root))
}

/// The workspace-relative `Cargo.toml` of the Cargo workspace the package at `manifest` belongs to, read from the working
/// tree. A package that no workspace governs is a workspace of its own, which is where its `Cargo.lock` is written.
///
/// # Errors
///
/// A manifest on the way up that cannot be read or parsed, or an unusable `package.workspace`.
pub fn cargo_workspace_of(
    reads: &mut WorkspaceReads,
    workspace_root: &Path,
    manifest: &Path,
) -> Result<PathBuf, ManifestError> {
    let manifest = &workspace_relative(manifest).map_err(|error| ManifestError::Read {
        path: manifest.to_path_buf(),
        message: error.to_string(),
    })?;
    let text = read_manifest_text(workspace_root)(manifest)?.ok_or_else(|| ManifestError::Read {
        path: manifest.to_path_buf(),
        message: "No such file or directory".to_string(),
    })?;
    let doc = parse_manifest(manifest, &text)?;
    Ok(reads
        .enclosing(manifest, &doc, read_manifest_text(workspace_root))?
        .unwrap_or_else(|| manifest.to_path_buf()))
}

/// Every manifest that could be the workspace root of the package at `manifest`: each `Cargo.toml` above its directory,
/// and the one its working-tree `package.workspace` names. Fetch these from a commit before asking
/// [`WorkspaceReads::enclosing`] about it there.
///
/// # Errors
///
/// The package's own manifest cannot be read or parsed.
pub fn workspace_manifest_candidates(workspace_root: &Path, manifest: &Path) -> Result<Vec<PathBuf>, ManifestError> {
    let dir = manifest.parent().unwrap_or(Path::new(""));
    let mut candidates: Vec<PathBuf> = dir
        .ancestors()
        .skip(1)
        .map(|ancestor| ancestor.join("Cargo.toml"))
        .collect();
    if let Some(text) = read_manifest_text(workspace_root)(manifest)? {
        candidates.extend(named_workspace_manifest(manifest, &text)?);
    }
    candidates.sort();
    candidates.dedup();
    Ok(candidates)
}

/// The workspace manifest that `text`, the content of the package manifest at `manifest`, names with
/// `package.workspace`, relative to the workspace root; `None` when it names none or the path leaves the root.
///
/// # Errors
///
/// `text` does not parse.
pub fn named_workspace_manifest(manifest: &Path, text: &str) -> Result<Option<PathBuf>, ManifestError> {
    let dir = manifest.parent().unwrap_or(Path::new(""));
    let doc = parse_manifest(manifest, text)?;
    Ok(doc
        .get("package")
        .and_then(|package| package.get("workspace"))
        .and_then(toml_edit::Item::as_str)
        .and_then(|named| workspace_relative(dir.join(named)).ok())
        .map(|root| root.join("Cargo.toml")))
}

/// Whether the manifest declares a workspace. Cargo treats any `workspace` table as one, however it is spelled: a
/// `[workspace]` header, an inline table, dotted keys, or one that exists only as `[workspace.package]`.
pub fn declares_workspace(doc: &toml_edit::DocumentMut) -> bool {
    doc.get("workspace").is_some_and(toml_edit::Item::is_table_like)
}

/// Parses the text of the `Cargo.toml` at `manifest`, which is used only to name it in the error.
pub fn parse_manifest(manifest: &Path, text: &str) -> Result<toml_edit::DocumentMut, ManifestError> {
    text.strip_prefix('\u{FEFF}')
        .unwrap_or(text)
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| ManifestError::Parse {
            path: manifest.to_path_buf(),
            format: ManifestFormat::CargoToml,
            message: error.to_string(),
        })
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

/// The metadata crates.io requires that `doc` leaves unset: `description`, and `license` or `license-file`. A field set
/// to `{ workspace = true }` is read from `workspace` (the governing `[workspace.package]`), and an empty string counts
/// as unset.
pub fn missing_crates_io_metadata(
    doc: &toml_edit::DocumentMut,
    workspace: Option<&toml_edit::DocumentMut>,
) -> Vec<&'static str> {
    let set = |key: &str| {
        let Some(value) = doc.get("package").and_then(|package| package.get(key)) else {
            return false;
        };
        let inherited = value.get("workspace").and_then(toml_edit::Item::as_bool) == Some(true);
        let value = if inherited {
            workspace
                .and_then(|workspace| workspace.get("workspace"))
                .and_then(|table| table.get("package"))
                .and_then(|package| package.get(key))
        } else {
            Some(value)
        };
        value
            .and_then(toml_edit::Item::as_str)
            .is_some_and(|text| !text.trim().is_empty())
    };
    let mut missing = Vec::new();
    if !set("description") {
        missing.push("description");
    }
    if !set("license") && !set("license-file") {
        missing.push("license");
    }
    missing
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
        if let Some(existing) = tbl.get_mut("version") {
            let value = existing.as_value_mut().ok_or(())?;
            let decor = value.decor().clone();
            let mut new_val = toml_edit::Value::from(new_str);
            *new_val.decor_mut() = decor;
            *value = new_val;
        } else {
            tbl.insert("version", toml_edit::value(new_str));
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

    fn bin_names(&self) -> Result<Vec<String>, ManifestError> {
        let Some(package) = cargo_package_name(&self.document) else {
            return Ok(Vec::new());
        };
        let unreadable = |path: &Path, error: std::io::Error| ManifestError::Read {
            path: self.path.parent().unwrap_or(Path::new("")).join(path),
            message: error.to_string(),
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
            let dir = self.absolute.parent().unwrap_or(Path::new("."));
            let is_file =
                |path: &Path| callisto_model::strict_fs::exists(path).map(|present| present && path.is_file());
            if is_file(&dir.join("src/main.rs")).map_err(|error| unreadable(Path::new("src/main.rs"), error))? {
                names.insert(package.to_owned());
            }
            // No `src/bin` is no extra binaries; one that cannot be read is an error.
            let entries = match fs::read_dir(dir.join("src/bin")) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(names.into_iter().collect()),
                Err(error) => return Err(unreadable(Path::new("src/bin"), error)),
            };
            for entry in entries {
                let path = entry.map_err(|error| unreadable(Path::new("src/bin"), error))?.path();
                if path.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
                    if let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) {
                        names.insert(name.to_owned());
                    }
                } else if path.is_dir()
                    && is_file(&path.join("main.rs")).map_err(|error| unreadable(Path::new("src/bin"), error))?
                    && let Some(name) = path.file_name().and_then(|name| name.to_str())
                {
                    names.insert(name.to_owned());
                }
            }
        }
        Ok(names.into_iter().collect())
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
            if let Some(ref inh) = self.inheritance
                && let Some(ref v) = inh.version
            {
                return Ok(v.clone());
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
            // The comment above the entry and the one after it belong to the line being replaced. In the dotted form
            // (`version.workspace = true`) they hang off `version` and `workspace`, so both are read.
            let mut prefix = pkg.key("version").and_then(|key| key.leaf_decor().prefix()).cloned();
            let mut suffix = None;
            if let Some(item) = pkg.get("version") {
                // A value (`version = { workspace = true }`) carries its trailing comment itself; the dotted form is a
                // table whose trailing comment is on the `workspace` value inside it.
                if let Some(value) = item.as_value() {
                    suffix = value.decor().suffix().cloned();
                } else if let Some(inner) = item.as_table_like() {
                    prefix = prefix.or_else(|| {
                        inner
                            .get_key_value("workspace")
                            .and_then(|(key, _)| key.leaf_decor().prefix().cloned())
                    });
                    suffix = inner
                        .get("workspace")
                        .and_then(|value| value.as_value())
                        .and_then(|value| value.decor().suffix().cloned());
                }
            }
            pkg.insert("version", toml_edit::value(v.render()));
            if let Some(mut key) = pkg.key_mut("version")
                && let Some(prefix) = prefix
            {
                key.leaf_decor_mut().set_prefix(prefix);
            }
            if let (Some(value), Some(suffix)) = (pkg.get_mut("version").and_then(|item| item.as_value_mut()), suffix) {
                value.decor_mut().set_suffix(suffix);
            }
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

    fn inherited_dependency(&self, name: &str) -> Option<InheritedDep<'_>> {
        self.inheritance.as_ref()?.inherited(name)
    }

    fn iter_dependencies(&self) -> Box<dyn Iterator<Item = DependencyEntry> + '_> {
        let mut entries = Vec::new();

        for (section_name, section_kind, target) in dependency_locations(&self.document) {
            if let Some(table) = dependency_table(&self.document, section_name, target.as_deref()) {
                for (name, item) in table.iter() {
                    let is_inherited = is_inherited_item(item);
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
                        locator: DepLocator::cargo(section_kind, target.clone()),
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
        at: &DepLocator,
        new: DepSpec,
        permit: &ApplyPermit,
    ) -> Result<(), ManifestError> {
        let not_found = || ManifestError::DependencyNotFound {
            path: self.path.clone(),
            name: name.to_string(),
            kind,
        };
        if kind == DepKind::Peer {
            return Err(not_found());
        }

        let (section, target) = match at {
            DepLocator::Cargo { section, target, .. } => (section_name(*section), target.as_deref()),
            DepLocator::Unspecified => (section_name(kind), None),
            _ => return Err(not_found()),
        };
        let item = dependency_table(&self.document, section, target)
            .and_then(|table| table.get(name))
            .ok_or_else(not_found)?;

        if is_inherited_item(item) {
            let root_manifest = self.inheritance.as_ref().ok_or_else(not_found)?.root_manifest.clone();
            let root_cargo = self.workspace_root.join(&root_manifest);
            if self.absolute == root_cargo {
                return Err(ManifestError::InvariantViolation {
                    path: self.path.clone(),
                    message: format!(
                        "dependency `{name}` is workspace-inherited but this manifest IS the workspace root (`{}`); refusing to delegate to avoid overwriting the just-written root manifest with a stale in-memory document",
                        root_cargo.display()
                    ),
                });
            }
            let mut ws_res = WorkspaceCargoResolver::load_at(&self.workspace_root, &root_manifest)?;
            ws_res.write_dependency(name, new, permit)?;
            return ws_res.persist(permit);
        }

        let item = dependency_table_mut(&mut self.document, section, target)
            .and_then(|table| table.get_mut(name))
            .ok_or_else(not_found)?;
        set_dependency_version(item, new.render()).map_err(|()| ManifestError::UnrecognizedDependencyValue {
            path: self.path.clone(),
            name: name.to_string(),
        })
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

const DEPENDENCY_SECTIONS: [(&str, DepKind); 3] = [
    ("dependencies", DepKind::Runtime),
    ("dev-dependencies", DepKind::Dev),
    ("build-dependencies", DepKind::Build),
];

/// The Cargo section a dependency kind is declared in; optional dependencies sit beside runtime ones.
fn section_name(kind: DepKind) -> &'static str {
    match kind {
        DepKind::Dev => "dev-dependencies",
        DepKind::Build => "build-dependencies",
        _ => "dependencies",
    }
}

/// Every dependency table a manifest can declare: the top-level ones, then the same three
/// under each `[target.<cfg>]` key. Owned, so callers can mutate the document while iterating.
fn dependency_locations(doc: &toml_edit::DocumentMut) -> Vec<(&'static str, DepKind, Option<String>)> {
    let mut locations: Vec<_> = DEPENDENCY_SECTIONS
        .iter()
        .map(|(section, kind)| (*section, *kind, None))
        .collect();
    if let Some(targets) = doc.get("target").and_then(|item| item.as_table_like()) {
        for (cfg, _) in targets.iter() {
            for (section, kind) in DEPENDENCY_SECTIONS {
                locations.push((section, kind, Some(cfg.to_string())));
            }
        }
    }
    locations
}

fn dependency_table<'a>(
    doc: &'a toml_edit::DocumentMut,
    section: &str,
    target: Option<&str>,
) -> Option<&'a dyn toml_edit::TableLike> {
    let parent = match target {
        Some(cfg) => doc.get("target")?.get(cfg)?,
        None => doc.as_item(),
    };
    parent.get(section)?.as_table_like()
}

fn dependency_table_mut<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    section: &str,
    target: Option<&str>,
) -> Option<&'a mut dyn toml_edit::TableLike> {
    let parent = match target {
        Some(cfg) => doc.get_mut("target")?.get_mut(cfg)?,
        None => doc.as_item_mut(),
    };
    parent.get_mut(section)?.as_table_like_mut()
}

/// True when the declaration is `{ workspace = true }`.
fn is_inherited_item(item: &toml_edit::Item) -> bool {
    item.as_inline_table()
        .and_then(|table| table.get("workspace"))
        .and_then(|workspace| workspace.as_bool())
        == Some(true)
        || item
            .as_table()
            .and_then(|table| table.get("workspace"))
            .and_then(|workspace| workspace.as_bool())
            == Some(true)
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
    if crate::common::is_bare_semver(s)
        && let Ok(v) = Version::parse(s, VersionGrammar::SemVer)
    {
        return DepSpec::CargoBare(v);
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
    /// Loads the Cargo workspace root manifest at `workspace_root/root_manifest`, keeping `root_manifest` (relative to
    /// `workspace_root`) as the path every report, diff and staged file names it by.
    pub fn load_at(workspace_root: &Path, root_manifest: &Path) -> Result<Self, ManifestError> {
        let rel_path = workspace_relative(root_manifest).map_err(|e| ManifestError::Read {
            path: root_manifest.to_path_buf(),
            message: e.to_string(),
        })?;
        let absolute_path = workspace_root.join(&rel_path);
        Self::read_from(rel_path, absolute_path)
    }

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
        Self::read_from(rel_path, root_manifest_path.to_path_buf())
    }

    fn read_from(rel_path: PathBuf, absolute_path: PathBuf) -> Result<Self, ManifestError> {
        crate::record_resolver_load_call();
        let content = fs::read_to_string(&absolute_path).map_err(|e| ManifestError::Read {
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
            absolute_path,
            document: doc,
        })
    }

    pub fn inheritance(&self) -> Result<WorkspaceInheritance, ManifestError> {
        let version = workspace_package_version(&self.document, &self.root_path)?;

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
                        locator: DepLocator::default(),
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
