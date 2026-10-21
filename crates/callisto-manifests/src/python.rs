use std::fs;
use std::path::{Path, PathBuf};

use callisto_model::{
    normalize_pypi_package_name, ApplyPermit, DepContainer, DepKind, DepLocator, DepSpec, DependencyEntry, Ecosystem,
    ManifestDecl, ManifestError, ManifestFormat, ManifestRole, PublishTarget, Version, VersionGrammar, VersionReq,
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
    fingerprint: crate::common::FormatFingerprint,
}

impl PyprojectToml {
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
                    format: ManifestFormat::PyprojectToml,
                    message: e.to_string(),
                })?;

        Ok(Self {
            path: rel_path,
            absolute: abs_path,
            role: decl.role.clone(),
            document: doc,
            fingerprint,
        })
    }

    fn render(&self) -> String {
        self.fingerprint.apply(&self.document.to_string())
    }
}

/// Extracts a package name from an already-parsed `pyproject.toml`
/// document, checking (1) PEP 621 `[project].name`, (2) Poetry
/// `[tool.poetry].name`, (3) Flit `[tool.flit.metadata].module`, in that
/// order. Pure and I/O-free -- see `crate::cargo::cargo_package_name`'s
/// doc comment for why this exists as a standalone function rather than
/// only living inside [`Manifest::package_name`].
pub fn python_package_name(doc: &toml_edit::DocumentMut) -> Option<&str> {
    doc.get("project")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .or_else(|| {
            doc.get("tool")
                .and_then(|t| t.get("poetry"))
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
        })
        .or_else(|| {
            doc.get("tool")
                .and_then(|t| t.get("flit"))
                .and_then(|f| f.get("metadata"))
                .and_then(|m| m.get("module"))
                .and_then(|n| n.as_str())
        })
}

/// [`crate::read_identity`]'s `pyproject.toml` implementation: parses
/// `source` and extracts a package name (PEP 621, falling back to Poetry
/// then Flit -- see [`python_package_name`]) plus a
/// [`crate::VersionSource::Literal`] version (PEP 621 `project.version`,
/// falling back to Poetry's `tool.poetry.version`). Python has no
/// workspace-inherited-version concept analogous to Cargo's, and a Flit or
/// PEP 621 `dynamic = ["version"]` package has no static version to report
/// at all, so both fall through to `None`.
pub(crate) fn identity_from_source(source: &str, path: &Path) -> Result<crate::ManifestIdentity, ManifestError> {
    let clean = source.strip_prefix('\u{FEFF}').unwrap_or(source);
    let doc: toml_edit::DocumentMut = clean.parse().map_err(|e: toml_edit::TomlError| ManifestError::Parse {
        path: path.to_path_buf(),
        format: ManifestFormat::PyprojectToml,
        message: e.to_string(),
    })?;

    let name = python_package_name(&doc).map(str::to_string);
    let version = doc
        .get("project")
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            doc.get("tool")
                .and_then(|t| t.get("poetry"))
                .and_then(|p| p.get("version"))
                .and_then(|v| v.as_str())
        })
        .map(|s| crate::VersionSource::Literal(s.to_string()));

    Ok(crate::ManifestIdentity { name, version })
}

/// A parsed PEP 508 dependency specifier, e.g.
/// `"requests[security,socks]>=2.0; python_version >= '3.8'"`.
///
/// Previously decomposed independently at three call sites in this module
/// (`iter_dependencies`, `update_dependency_spec`,
/// `update_optional_dependencies`), each hand-rolling the same
/// marker-split / operator-split / extras-split sequence -- this type is
/// the single implementation all three now share.
///
/// `render` produces a normalized textual form (no space around `[...]`,
/// extras comma-joined with no interior space, no space between the
/// name/extras and the version spec, and a single `;` directly before the
/// marker with no extra whitespace). This is not necessarily byte-identical
/// to arbitrary input formatting, but `Requirement::parse(&r.render())`
/// always reproduces a `Requirement` equal to `r` when `r` itself came from
/// `parse` -- see the `requirement_parse_render_parse_is_idempotent`
/// property test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requirement {
    pub name: String,
    pub extras: Vec<String>,
    /// The version-constraint clause(s) verbatim (e.g. `">=1.0,<2.0"` or
    /// `"==1.2.3"`) -- `None` when the requirement carries no version
    /// constraint at all (a bare name, optionally with extras/marker).
    pub spec: Option<String>,
    /// The marker expression (without the leading `;` or surrounding
    /// whitespace), e.g. `"python_version >= '3.8'"` -- `None` when absent.
    pub marker: Option<String>,
}

impl Requirement {
    /// Parses a single PEP 508 requirement string. Returns `None` when no
    /// package name can be extracted (e.g. an empty string, or a string
    /// that is entirely a marker/version clause with nothing before it).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();

        let mut halves = s.splitn(2, ';');
        let spec_part = halves.next().unwrap_or("").trim();
        let marker = halves
            .next()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_string);

        let op_idx = spec_part.find(['<', '>', '=', '!', '~']);
        let (pkg_part, spec) = match op_idx {
            Some(idx) => (spec_part[..idx].trim(), Some(spec_part[idx..].trim().to_string())),
            None => (spec_part, None),
        };

        let (name, extras) = match pkg_part.find('[') {
            Some(idx) => {
                let name = pkg_part[..idx].trim().to_string();
                let extras_str = pkg_part[idx..].trim_start_matches('[').trim_end_matches(']');
                let extras = extras_str
                    .split(',')
                    .map(str::trim)
                    .filter(|e| !e.is_empty())
                    .map(str::to_string)
                    .collect();
                (name, extras)
            }
            None => (pkg_part.trim().to_string(), Vec::new()),
        };

        if name.is_empty() {
            return None;
        }

        Some(Requirement {
            name,
            extras,
            spec,
            marker,
        })
    }

    /// Renders back to PEP 508 text -- see the struct's doc comment for the
    /// normalization this applies relative to arbitrary input formatting.
    pub fn render(&self) -> String {
        let mut out = self.name.clone();
        if !self.extras.is_empty() {
            out.push('[');
            out.push_str(&self.extras.join(","));
            out.push(']');
        }
        if let Some(spec) = &self.spec {
            out.push_str(spec);
        }
        if let Some(marker) = &self.marker {
            out.push(';');
            out.push_str(marker);
        }
        out
    }
}

impl std::fmt::Display for Requirement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render())
    }
}

/// Splits a bare `major[.minor[.patch]]` version core (no pre-release/local
/// suffix, at most three dotted integer components) into its components.
/// Returns `None` for anything else -- a caret/tilde/wildcard clause whose
/// core doesn't fit this shape (e.g. `^1.2.3-beta`) is not normalized here;
/// the caller falls through to a raw PEP 440 parse attempt (which will fail)
/// and ultimately an opaque fallback, rather than mis-normalizing it.
fn parse_version_core(s: &str) -> Option<(u64, Option<u64>, Option<u64>)> {
    let mut parts = s.split('.');
    let major: u64 = parts.next()?.parse().ok()?;
    let minor = parts.next().map(str::parse).transpose().ok()?;
    let patch = parts.next().map(str::parse).transpose().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Expands a Poetry caret clause's version core (the part after `^`) into an
/// equivalent PEP 440 range. Caret allows changes that do not modify the
/// leftmost non-zero component of `major.minor.patch` -- see
/// <https://python-poetry.org/docs/dependency-specification/#caret-requirements>.
fn normalize_caret(rest: &str) -> Option<String> {
    let (major, minor, patch) = parse_version_core(rest)?;
    let lower = format!("{major}.{}.{}", minor.unwrap_or(0), patch.unwrap_or(0));

    let upper = if major > 0 {
        format!("{}.0.0", major + 1)
    } else if let Some(min) = minor {
        if min > 0 {
            format!("0.{}.0", min + 1)
        } else if let Some(pat) = patch {
            format!("0.0.{}", pat + 1)
        } else {
            "0.1.0".to_string()
        }
    } else {
        "1.0.0".to_string()
    };

    Some(format!(">={lower},<{upper}"))
}

/// Expands a Poetry tilde clause's version core (the part after `~`) into an
/// equivalent PEP 440 range. Tilde allows patch-level changes when a minor
/// version is given, otherwise major-level changes -- see
/// <https://python-poetry.org/docs/dependency-specification/#tilde-requirements>.
fn normalize_tilde(rest: &str) -> Option<String> {
    let (major, minor, patch) = parse_version_core(rest)?;
    let lower = format!("{major}.{}.{}", minor.unwrap_or(0), patch.unwrap_or(0));
    let upper = match minor {
        Some(min) => format!("{major}.{}.0", min + 1),
        None => format!("{}.0.0", major + 1),
    };
    Some(format!(">={lower},<{upper}"))
}

/// Expands a bare Poetry wildcard clause (`1.*`, `1.2.*`) into an equivalent
/// PEP 440 range. Does not handle wildcards already attached to an explicit
/// operator (`==1.2.*`, `!=1.2.*`) -- those are valid PEP 440 as-is and must
/// not reach this function (see `normalize_poetry_clause`).
fn normalize_wildcard(clause: &str) -> Option<String> {
    let core = clause.strip_suffix(".*")?;
    if core.is_empty() {
        return None;
    }
    let mut parts = core.split('.');
    let major: u64 = parts.next()?.parse().ok()?;
    match parts.next() {
        None => Some(format!(">={major}.0.0,<{}.0.0", major + 1)),
        Some(minor_str) => {
            if parts.next().is_some() {
                return None;
            }
            let minor: u64 = minor_str.parse().ok()?;
            Some(format!(">={major}.{minor}.0,<{major}.{}.0", minor + 1))
        }
    }
}

/// Normalizes a single comma-separated clause of a Poetry version
/// constraint. Only bare caret/tilde/wildcard clauses are rewritten; a
/// clause that's already PEP 440-compatible (`>=1.2`, `~=1.2.3`, `==1.2.*`)
/// passes through unchanged, since Poetry accepts standard PEP 440 syntax
/// verbatim alongside its own caret/tilde/wildcard extensions.
fn normalize_poetry_clause(clause: &str) -> String {
    if clause == "*" {
        return String::new();
    }
    if let Some(rest) = clause.strip_prefix('^') {
        if let Some(normalized) = normalize_caret(rest) {
            return normalized;
        }
    } else if let Some(rest) = clause.strip_prefix('~') {
        // Distinguish Poetry's bare tilde ("~1.2.3") from PEP 440's
        // compatible-release operator ("~=1.2.3"), which is already valid.
        if !rest.starts_with('=')
            && let Some(normalized) = normalize_tilde(rest)
        {
            return normalized;
        }
    } else if clause.starts_with(|c: char| c.is_ascii_digit())
        && clause.contains('*')
        && let Some(normalized) = normalize_wildcard(clause)
    {
        return normalized;
    }
    clause.to_string()
}

/// Normalizes a Poetry-style version constraint -- which allows caret (`^`),
/// tilde (`~`), and wildcard (`*`) syntax on top of PEP 440, see
/// <https://python-poetry.org/docs/dependency-specification/#version-constraints>
/// -- into a comma-joined PEP 440 range string that `pep440_rs` can parse
/// directly.
///
/// Each comma-separated clause is normalized independently, so compound
/// constraints like `^1.2,!=1.2.5` are handled correctly. A clause this
/// normalizer doesn't recognize (or can't decompose, e.g. a caret applied to
/// a pre-release version) passes through unchanged; the subsequent
/// `VersionReq::parse` attempt then fails on it, and the caller falls back
/// to an opaque dependency entry rather than silently dropping the
/// dependency.
fn normalize_poetry_version_spec(raw: &str) -> String {
    raw.split(',')
        .map(str::trim)
        .filter(|clause| !clause.is_empty())
        .map(normalize_poetry_clause)
        .filter(|clause| !clause.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

impl Manifest for PyprojectToml {
    fn persist(&mut self, permit: &ApplyPermit) -> Result<(), ManifestError> {
        let content = self.render();
        atomic_write(&self.absolute, &content, permit).map_err(|e| ManifestError::Write {
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
        Ecosystem::Pypi
    }

    fn role(&self) -> ManifestRole {
        self.role.clone()
    }

    fn bin_names(&self) -> Result<Vec<String>, ManifestError> {
        Ok(self
            .document
            .get("project")
            .and_then(|project| project.get("scripts"))
            .and_then(|scripts| scripts.as_table_like())
            .map(|scripts| scripts.iter().map(|(name, _)| name.to_string()).collect())
            .unwrap_or_default())
    }

    fn package_name(&self) -> Result<String, ManifestError> {
        python_package_name(&self.document)
            .map(|s| s.to_string())
            .ok_or_else(|| ManifestError::MissingField {
                path: self.path.clone(),
                field: "project.name / tool.poetry.name / tool.flit.metadata.module",
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

    fn write_version(&mut self, v: &Version, _permit: &ApplyPermit) -> Result<(), ManifestError> {
        let new_ver = v.render();

        let has_project_version = self.document.get("project").and_then(|p| p.get("version")).is_some();

        if has_project_version {
            if let Some(proj) = self.document.get_mut("project").and_then(|p| p.as_table_like_mut()) {
                crate::common::set_scalar_preserving_decor(proj, "version", new_ver);
            }
        } else if let Some(poetry) = self
            .document
            .get_mut("tool")
            .and_then(|t| t.get_mut("poetry"))
            .and_then(|p| p.as_table_like_mut())
        {
            crate::common::set_scalar_preserving_decor(poetry, "version", new_ver);
        } else if let Some(flit) = self
            .document
            .get_mut("tool")
            .and_then(|t| t.get_mut("flit"))
            .and_then(|f| f.get_mut("metadata"))
            .and_then(|m| m.as_table_like_mut())
        {
            crate::common::set_scalar_preserving_decor(flit, "version", new_ver);
        } else {
            self.document["project"]["version"] = value(new_ver);
        }

        Ok(())
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
            entries.extend(requirement_entries(
                deps,
                DepKind::Runtime,
                &DepContainer::ProjectDependencies,
            ));
        }

        // 1b. PEP 621 [project.optional-dependencies]: extras ship in the published metadata, so a
        // pin there goes stale exactly like a runtime one.
        if let Some(extras) = self
            .document
            .get("project")
            .and_then(|p| p.get("optional-dependencies"))
            .and_then(|d| d.as_table_like())
        {
            for (extra, group) in extras.iter() {
                if let Some(requirements) = group.as_array() {
                    entries.extend(requirement_entries(
                        requirements,
                        DepKind::Optional,
                        &DepContainer::Extra(extra.to_string()),
                    ));
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
                // `as_table_like()` covers both inline tables
                // (`foo = { version = "^1.2" }`) and dotted-table syntax
                // (`[tool.poetry.dependencies.foo]` / `version = "^1.2"`),
                // which parse to different `toml_edit::Item` variants but
                // must both be recognized -- previously only the inline-table
                // variant was handled, so a dotted-table Poetry dependency
                // silently vanished from the graph entirely.
                let raw_req = match item {
                    toml_edit::Item::Value(toml_edit::Value::String(s)) => Some(s.value().as_str()),
                    _ => item
                        .as_table_like()
                        .and_then(|t| t.get("version"))
                        .and_then(|v| v.as_str()),
                };

                let spec = match raw_req {
                    Some(req_str) => {
                        // Poetry allows caret/tilde/wildcard syntax on top of
                        // PEP 440 (e.g. `^1.2`, `~1.2.3`, `1.*`), which
                        // `pep440_rs` cannot parse directly -- normalize it
                        // to an equivalent PEP 440 range first.
                        let normalized = normalize_poetry_version_spec(req_str);
                        match VersionReq::parse(&normalized, Ecosystem::Pypi) {
                            Ok(spec) => DepSpec::Range(spec, req_str.to_string()),
                            // Genuinely unrepresentable (syntax this
                            // normalizer doesn't cover) -- never silently
                            // drop the dependency; keep it visible in the
                            // graph as an opaque spec instead.
                            Err(_) => DepSpec::Opaque(req_str.to_string()),
                        }
                    }
                    // No `version` key at all -- a path/git/url dependency
                    // (e.g. `foo = { git = "...", tag = "v1.0" }`) or an
                    // unrecognized value shape. Still must not silently
                    // vanish from the graph.
                    None => {
                        DepSpec::Opaque("non-versioned dependency (path/git/url or unrecognized shape)".to_string())
                    }
                };

                entries.push(DependencyEntry {
                    name: name.to_string(),
                    package_name: name.to_string(),
                    inherited: false,
                    kind: DepKind::Runtime,
                    spec,
                    locator: DepLocator::python(DepContainer::PoetryDependencies, None),
                });
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
        _permit: &ApplyPermit,
    ) -> Result<(), ManifestError> {
        let new_spec_str = match new {
            DepSpec::Range(req, _) => req.render().to_string(),
            DepSpec::Exact(v) => format!("=={}", v.render()),
            _ => return Ok(()),
        };
        let not_found = || ManifestError::DependencyNotFound {
            path: self.path.clone(),
            name: name.to_string(),
            kind,
        };

        let DepLocator::Python {
            container, position, ..
        } = at
        else {
            return Err(not_found());
        };
        match container {
            DepContainer::PoetryDependencies => {
                let table = self
                    .document
                    .get_mut("tool")
                    .and_then(|t| t.get_mut("poetry"))
                    .and_then(|p| p.get_mut("dependencies"))
                    .and_then(|d| d.as_table_like_mut())
                    .ok_or_else(not_found)?;
                let existing = table.get_mut(name).ok_or_else(not_found)?;
                match existing.as_table_like_mut() {
                    Some(fields) => {
                        fields.insert("version", value(new_spec_str));
                    }
                    None => {
                        table.insert(name, value(new_spec_str));
                    }
                }
            }
            container => {
                let position = position.ok_or_else(not_found)?;
                let requirements = match container {
                    DepContainer::ProjectDependencies => {
                        self.document.get_mut("project").and_then(|p| p.get_mut("dependencies"))
                    }
                    DepContainer::Extra(extra) => self
                        .document
                        .get_mut("project")
                        .and_then(|p| p.get_mut("optional-dependencies"))
                        .and_then(|d| d.get_mut(extra.as_str())),
                    _ => None,
                }
                .and_then(|item| item.as_array_mut())
                .ok_or_else(not_found)?;
                // The array may differ from the one the locator was read from: refuse to rewrite another dependency.
                let rewritten = requirements
                    .get(position)
                    .and_then(|item| item.as_str())
                    .and_then(|full_req| rewrite_requirement(full_req, name, &new_spec_str))
                    .ok_or_else(not_found)?;
                requirements.replace(position, rewritten);
            }
        }

        Ok(())
    }

    fn is_publishable(&self) -> bool {
        // A pyproject.toml without [project].name or [tool.poetry].name is a
        // workspace root or build-system-only file — not a publishable package.
        let has_pep621_name = self
            .document
            .get("project")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .is_some();
        let has_poetry_name = self
            .document
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .is_some();
        has_pep621_name || has_poetry_name
    }

    fn publish_targets(&self) -> Vec<PublishTarget> {
        if !self.is_publishable() {
            return vec![PublishTarget::None];
        }
        vec![PublishTarget::Pypi { index: None }]
    }

    fn update_optional_dependencies(
        &mut self,
        updates: &[(String, Version)],
        _permit: &ApplyPermit,
    ) -> Result<(), ManifestError> {
        if updates.is_empty() {
            return Ok(());
        }

        // [project.optional-dependencies] is a table of group_name → [PEP 508 strings].
        // Collect the table keys first to avoid borrow conflicts.
        let group_keys: Vec<String> = self
            .document
            .get("project")
            .and_then(|p| p.get("optional-dependencies"))
            .and_then(|od| od.as_table_like())
            .map(|t| t.iter().map(|(k, _)| k.to_string()).collect())
            .unwrap_or_default();

        for group_key in &group_keys {
            let Some(arr) = self
                .document
                .get_mut("project")
                .and_then(|p| p.get_mut("optional-dependencies"))
                .and_then(|od| od.get_mut(group_key.as_str()))
                .and_then(|g| g.as_array_mut())
            else {
                continue;
            };

            for idx in 0..arr.len() {
                let Some(full_req) = arr.get(idx).and_then(|item| item.as_str()) else {
                    continue;
                };
                let Some(req) = Requirement::parse(full_req) else {
                    continue;
                };

                if let Some((_dep_name, new_ver)) = updates.iter().find(|(dep_name, _)| {
                    normalize_pypi_package_name(dep_name) == normalize_pypi_package_name(&req.name)
                }) {
                    let new_req = Requirement {
                        name: req.name.clone(),
                        extras: req.extras.clone(),
                        spec: Some(format!(">={}", new_ver.render())),
                        marker: req.marker.clone(),
                    };
                    arr.replace(idx, new_req.render());
                }
            }
        }

        Ok(())
    }
}

/// Rewrites a PEP 508 dependency constraint to pin it to `target`.
///
/// The following single-clause and two-clause forms are recognized and rewritten:
///
/// - `==X.Y.Z` — exact pin — is rewritten to `==NEW`.
/// - `~=X.Y.Z` — compatible release — is rewritten to `~=NEW`.
/// - `>=X.Y.Z` — lower bound only — is rewritten to `>=NEW`.
/// - `>=X.Y.Z,<A.B` — two-clause range — is rewritten to `>=NEW,<NEXT_MAJOR`
///   where `NEXT_MAJOR` equals `target.major() + 1`.
///
/// A pre-release or dev/local target (e.g. a `snapshot` version) is rendered
/// in full rather than truncated to `major.minor.patch`, the same precision
/// rule `cargo::round_trip` applies for SemVer.
///
/// All other forms return `None` — including upper-bound-only (`<`, `<=`),
/// exclusion (`!=`), compound expressions beyond the two-clause range above,
/// and wildcard `*`. Callers must leave the original constraint unchanged
/// when `None` is returned.
pub fn round_trip(spec: &DepSpec, target: &Version) -> Option<DepSpec> {
    // Only Range specs are produced by iter_dependencies for Python.
    let original = match spec {
        DepSpec::Range(_, raw) => raw.as_str(),
        _ => return None,
    };

    let trimmed = original.trim();

    // Compound range: ">=X.Y.Z,<A.B" → ">=NEW,<NEXT_MAJOR"
    if trimmed.contains(',') {
        return rewrite_range(trimmed, target);
    }

    // Single-clause forms matched by their PEP 508 operator prefix.
    if trimmed.starts_with("==") {
        return rewrite_single(trimmed, "==", target);
    }
    if trimmed.starts_with("~=") {
        return rewrite_single(trimmed, "~=", target);
    }
    if trimmed.starts_with(">=") {
        return rewrite_single(trimmed, ">=", target);
    }
    // Poetry's own operators: bare caret and tilde (PEP 440's `~=` was handled above).
    if let Some(core) = trimmed.strip_prefix('^') {
        return rewrite_poetry_operator("^", core, target);
    }
    if let Some(core) = trimmed.strip_prefix('~').filter(|rest| !rest.starts_with('=')) {
        return rewrite_poetry_operator("~", core, target);
    }

    // Unknown or unsupported form (e.g. "<", "<=", "!=", bare "*").
    None
}

/// Rewrites a single-clause PEP 508 specifier, preserving the operator.
fn rewrite_single(original: &str, op: &str, target: &Version) -> Option<DepSpec> {
    // Validate that the original clause parses correctly (guards against
    // unknown version syntax that happens to start with a known prefix).
    let _rest = original.strip_prefix(op)?;
    let rendered = format!("{op}{}", render_target(op, target)?);
    let req = VersionReq::parse(&rendered, Ecosystem::Pypi).ok()?;
    Some(DepSpec::Range(req, rendered))
}

/// Rewrites a Poetry caret or tilde clause to the same window around the stable `target`; the caller writes it as the
/// equivalent PEP 440 range, which Poetry reads too. A pre-release target has no caret or tilde form, so it is left alone.
fn rewrite_poetry_operator(op: &str, core: &str, target: &Version) -> Option<DepSpec> {
    parse_version_core(core)?;
    if target.is_prerelease() {
        return None;
    }
    let rendered = format!("{op}{}", render_target(op, target)?);
    let req = VersionReq::parse(&normalize_poetry_version_spec(&rendered), Ecosystem::Pypi).ok()?;
    Some(DepSpec::Range(req, rendered))
}

/// Renders `target` at the precision a rewritten spec should carry, for use after `op`:
/// `major.minor.patch` for a stable release; the version in full for a pre-release or
/// dev target paired with `==`/`!=` (the only operators PEP 440 allows on a local version);
/// the version with its local segment stripped for every other operator, since PEP 440
/// forbids `>=`, `<=`, `>`, `<` and `~=` from carrying one.
fn render_target(op: &str, target: &Version) -> Option<String> {
    if target.is_prerelease() {
        let full = target.render();
        if op == "==" || op == "!=" {
            return Some(full.to_string());
        }
        return Some(full.split('+').next().unwrap_or(full).to_string());
    }
    Some(format!("{}.{}.{}", target.major()?, target.minor()?, target.patch()?))
}

/// Rewrites a two-clause `>=X.Y.Z,<A.B` range to `>=NEW,<NEXT_MAJOR`.
///
/// Returns `None` for anything that does not match exactly this two-clause
/// lower/upper pattern, including specs with three or more clauses (e.g.
/// `>=1.0,<2.0,!=1.5.0`) where silently dropping extra clauses would produce
/// a semantically incorrect constraint.
fn rewrite_range(original: &str, target: &Version) -> Option<DepSpec> {
    // Reject any spec with more than two comma-separated clauses: rewriting
    // would silently discard the extra clauses, changing the semantics.
    if original.split(',').count() != 2 {
        return None;
    }

    let mut parts = original.splitn(2, ',');
    let lower = parts.next()?.trim();
    let upper = parts.next()?.trim();

    // Lower clause must be `>=…`.
    if !lower.starts_with(">=") {
        return None;
    }
    // Upper clause must be strictly `<…` (not `<=`).
    if !upper.starts_with('<') || upper.starts_with("<=") {
        return None;
    }

    // A window of one minor (`>=0.1.0,<0.2`) stays one minor wide; any other upper bound becomes the next major.
    let lower_core = lower
        .strip_prefix(">=")
        .and_then(|version| parse_version_core(version.trim()));
    let upper_core = upper
        .strip_prefix('<')
        .and_then(|version| parse_version_core(version.trim()));
    let one_minor_window = matches!(
        (lower_core, upper_core),
        (Some((lower_major, Some(lower_minor), _)), Some((upper_major, Some(upper_minor), upper_patch)))
            if upper_major == lower_major && upper_minor == lower_minor + 1 && upper_patch.unwrap_or(0) == 0
    );
    let upper_bound = if one_minor_window {
        format!("{}.{}", target.major()?, target.minor()? + 1)
    } else {
        (target.major()? + 1).to_string()
    };
    let rendered = format!(">={},<{upper_bound}", render_target(">=", target)?);
    let req = VersionReq::parse(&rendered, Ecosystem::Pypi).ok()?;
    Some(DepSpec::Range(req, rendered))
}

/// Every requirement string of one array as a dependency edge addressed by its index in `container`.
fn requirement_entries<'a>(
    requirements: &'a toml_edit::Array,
    kind: DepKind,
    container: &'a DepContainer,
) -> impl Iterator<Item = DependencyEntry> + 'a {
    requirements.iter().enumerate().filter_map(move |(position, item)| {
        let locator = DepLocator::python(container.clone(), Some(position));
        pep508_entry(item.as_str()?, kind, locator)
    })
}

/// One PEP 508 requirement string as a dependency edge, or `None` when it names no package.
fn pep508_entry(full_req_str: &str, kind: DepKind, locator: DepLocator) -> Option<DependencyEntry> {
    // `Requirement::parse` couldn't extract even a package name (an empty string or a
    // marker-only fragment): there is no name to key an entry on.
    let req = Requirement::parse(full_req_str)?;
    let req_str = req.spec.clone().unwrap_or_else(|| "*".to_string());
    let parsed_req_str = req.spec.as_deref().unwrap_or(">=0.0.0");
    let spec = match VersionReq::parse(parsed_req_str, Ecosystem::Pypi) {
        Ok(spec) => DepSpec::Range(spec, req_str),
        // A PEP 508 clause `pep440_rs` can't parse (exotic or malformed syntax): keep the
        // dependency visible in the graph rather than silently dropping it.
        Err(_) => DepSpec::Opaque(req_str),
    };
    Some(DependencyEntry {
        name: req.name.clone(),
        package_name: req.name.clone(),
        inherited: false,
        kind,
        spec,
        locator,
    })
}

/// The requirement string with `name`'s constraint replaced by `new_spec_str`, or `None` when
/// the requirement names another package.
fn rewrite_requirement(full_req: &str, name: &str, new_spec_str: &str) -> Option<String> {
    let req = Requirement::parse(full_req)?;
    if normalize_pypi_package_name(&req.name) != normalize_pypi_package_name(name) {
        return None;
    }
    let formatted_spec = if new_spec_str.starts_with(['<', '>', '=', '!', '~']) {
        new_spec_str.to_string()
    } else {
        format!(">={new_spec_str}")
    };
    Some(
        Requirement {
            name: req.name.clone(),
            extras: req.extras.clone(),
            spec: Some(formatted_spec),
            marker: req.marker.clone(),
        }
        .render(),
    )
}

#[cfg(test)]
mod tests;
