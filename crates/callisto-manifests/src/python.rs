use std::fs;
use std::path::{Path, PathBuf};

use callisto_model::{
    ApplyPermit, DepKind, DepSpec, DependencyEntry, Ecosystem, ManifestDecl, ManifestError,
    ManifestFormat, ManifestRole, PublishTarget, Version, VersionGrammar, VersionReq,
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
    has_bom: bool,
    line_ending: LineEnding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineEnding {
    Lf,
    CrLf,
}

impl PyprojectToml {
    pub fn open(decl: &ManifestDecl, ctx: &OpenContext<'_>) -> Result<Self, ManifestError> {
        let rel_path = decl.path.clone();
        let abs_path = ctx.workspace_root.join(&rel_path);

        let content = fs::read_to_string(&abs_path).map_err(|e| ManifestError::Read {
            path: rel_path.clone(),
            message: e.to_string(),
        })?;

        let has_bom = content.starts_with('\u{FEFF}');
        let clean_content = content.strip_prefix('\u{FEFF}').unwrap_or(&content);
        let line_ending = if clean_content.contains("\r\n") {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        };

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
            has_bom,
            line_ending,
        })
    }

    fn render(&self) -> String {
        let mut out = self.document.to_string();
        if self.line_ending == LineEnding::CrLf {
            out = out.replace("\r\n", "\n").replace('\n', "\r\n");
        }
        if self.has_bom {
            out = format!("\u{FEFF}{}", out);
        }
        out
    }
}

impl Manifest for PyprojectToml {
    fn persist(&mut self, permit: &ApplyPermit) -> Result<(), ManifestError> {
        let content = self.render();
        atomic_write(&self.absolute, &content, permit).map_err(|e| ManifestError::Write {
            path: self.path.clone(),
            message: e.to_string(),
        })
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

    fn write_version(&mut self, v: &Version, _permit: &ApplyPermit) -> Result<(), ManifestError> {
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

        let has_project_version = self
            .document
            .get("project")
            .and_then(|p| p.get("version"))
            .is_some();

        if has_project_version {
            if let Some(proj) = self.document.get_mut("project") {
                set_with_decor(proj, "version");
            }
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
        kind: DepKind,
        new: DepSpec,
        _permit: &ApplyPermit,
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

        if !updated {
            return Err(ManifestError::DependencyNotFound {
                path: self.path.clone(),
                name: name.to_string(),
                kind,
            });
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
                let full_req = full_req.to_string();
                let spec_part = full_req.split(';').next().unwrap_or(&full_req).trim();
                let op_idx = spec_part.find(&['<', '>', '=', '!', '~'][..]);
                let pkg_part = match op_idx {
                    Some(i) => &spec_part[..i],
                    None => spec_part,
                };
                let pkg_name = pkg_part.split('[').next().unwrap_or(pkg_part).trim();

                if let Some((_dep_name, new_ver)) = updates
                    .iter()
                    .find(|(dep_name, _)| dep_name.eq_ignore_ascii_case(pkg_name))
                {
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
                    let rendered_ver = new_ver.render();
                    let new_req = format!("{pkg_name}{extras}>={rendered_ver}{marker}");
                    arr.replace(idx, new_req);
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
/// All other forms return `None` — including upper-bound-only (`<`, `<=`),
/// exclusion (`!=`), compound expressions beyond the two-clause range above,
/// wildcard `*`, and pre-release targets.  Callers must leave the original
/// constraint unchanged when `None` is returned.
pub fn round_trip(spec: &DepSpec, target: &Version) -> Option<DepSpec> {
    // Only Range specs are produced by iter_dependencies for Python.
    let original = match spec {
        DepSpec::Range(_, raw) => raw.as_str(),
        _ => return None,
    };

    // Pre-release targets are not safe to rewrite automatically.
    if target.is_prerelease() {
        return None;
    }

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

    // Unknown or unsupported form (e.g. "<", "<=", "!=", bare "*").
    None
}

/// Rewrites a single-clause PEP 508 specifier, preserving the operator.
fn rewrite_single(original: &str, op: &str, target: &Version) -> Option<DepSpec> {
    // Validate that the original clause parses correctly (guards against
    // unknown version syntax that happens to start with a known prefix).
    let _rest = original.strip_prefix(op)?;
    let maj = target.major()?;
    let min = target.minor()?;
    let pat = target.patch()?;
    let rendered = format!("{op}{maj}.{min}.{pat}");
    let req = VersionReq::parse(&rendered, Ecosystem::Pypi).ok()?;
    Some(DepSpec::Range(req, rendered))
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

    let maj = target.major()?;
    let min = target.minor()?;
    let pat = target.patch()?;
    let next_major = maj + 1;
    let rendered = format!(">={maj}.{min}.{pat},<{next_major}");
    let req = VersionReq::parse(&rendered, Ecosystem::Pypi).ok()?;
    Some(DepSpec::Range(req, rendered))
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
        manifest.write_version(&new_v, &permit()).unwrap();
        manifest.persist(&permit()).unwrap();

        let updated_content = fs::read_to_string(&pyproject_path).unwrap();
        assert!(updated_content.contains("version = \"0.3.2\""));
        assert!(updated_content.contains("# Main project metadata"));
        assert!(updated_content.contains("# Current release version"));
    }

    #[test]
    fn write_version_does_not_touch_disk_until_persist_called() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");
        let content = "[project]\nname = \"my-python-lib\"\nversion = \"0.3.1\"\n";
        fs::write(&pyproject_path, content).unwrap();

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
        let new_v = Version::parse("0.3.2", VersionGrammar::Pep440).unwrap();
        manifest.write_version(&new_v, &permit()).unwrap();

        let unchanged = fs::read_to_string(&pyproject_path).unwrap();
        assert_eq!(
            unchanged, content,
            "write_version alone must not write to disk"
        );

        manifest.persist(&permit()).unwrap();
        let updated = fs::read_to_string(&pyproject_path).unwrap();
        assert!(updated.contains("version = \"0.3.2\""));
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
    fn preserves_bom_and_crlf_line_endings_on_write() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");

        fs::write(
            &pyproject_path,
            callisto_fixtures::corpus::pyproject_toml_bom_crlf_sample(),
        )
        .unwrap();

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
        assert_eq!(manifest.package_name().unwrap(), "bom-crlf-lib");

        let new_v = Version::parse("1.0.1", VersionGrammar::Pep440).unwrap();
        manifest.write_version(&new_v, &permit()).unwrap();
        manifest.persist(&permit()).unwrap();

        let updated_bytes = fs::read(&pyproject_path).unwrap();
        let updated = String::from_utf8(updated_bytes).unwrap();

        assert!(
            updated.starts_with('\u{FEFF}'),
            "expected UTF-8 BOM to survive write, got:\n{updated:?}"
        );
        assert!(
            updated.contains("\r\n"),
            "expected CRLF line endings to survive write, got:\n{updated:?}"
        );
        assert!(
            !updated.replace("\r\n", "").contains('\n'),
            "expected no bare LF line endings to remain, got:\n{updated:?}"
        );
        assert!(updated.contains("version = \"1.0.1\" # release version"));
    }

    #[test]
    fn preserves_crlf_line_endings_without_bom_on_write() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");

        fs::write(
            &pyproject_path,
            callisto_fixtures::corpus::pyproject_toml_crlf_no_bom_sample(),
        )
        .unwrap();

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
        assert_eq!(manifest.package_name().unwrap(), "crlf-lib");

        let new_v = Version::parse("1.0.1", VersionGrammar::Pep440).unwrap();
        manifest.write_version(&new_v, &permit()).unwrap();
        manifest.persist(&permit()).unwrap();

        let updated_bytes = fs::read(&pyproject_path).unwrap();
        let updated = String::from_utf8(updated_bytes).unwrap();

        assert!(
            !updated.starts_with('\u{FEFF}'),
            "expected no BOM to be introduced, got:\n{updated:?}"
        );
        assert!(
            updated.contains("\r\n"),
            "expected CRLF line endings to survive write, got:\n{updated:?}"
        );
        assert!(
            !updated.replace("\r\n", "").contains('\n'),
            "expected no bare LF line endings to remain, got:\n{updated:?}"
        );
        assert!(updated.contains("version = \"1.0.1\" # release version"));
    }

    #[test]
    fn preserves_bom_without_crlf_on_write() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");

        fs::write(
            &pyproject_path,
            callisto_fixtures::corpus::pyproject_toml_bom_no_crlf_sample(),
        )
        .unwrap();

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
        assert_eq!(manifest.package_name().unwrap(), "bom-lf-lib");

        let new_v = Version::parse("1.0.1", VersionGrammar::Pep440).unwrap();
        manifest.write_version(&new_v, &permit()).unwrap();
        manifest.persist(&permit()).unwrap();

        let updated_bytes = fs::read(&pyproject_path).unwrap();
        let updated = String::from_utf8(updated_bytes).unwrap();

        assert!(
            updated.starts_with('\u{FEFF}'),
            "expected UTF-8 BOM to survive write, got:\n{updated:?}"
        );
        assert!(
            !updated.replace('\u{FEFF}', "").contains("\r\n"),
            "expected no CRLF to be introduced, got:\n{updated:?}"
        );
        assert!(updated.contains("version = \"1.0.1\" # release version"));
    }

    #[test]
    fn empty_pyproject_toml_returns_parse_error_not_panic() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");
        fs::write(&pyproject_path, "").unwrap();

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

        // An empty TOML document is technically valid (empty table), so this
        // must not panic; the resulting manifest simply lacks required fields.
        let manifest = PyprojectToml::open(&decl, &ctx).unwrap();
        assert!(manifest.package_name().is_err());
    }

    #[test]
    fn bom_only_pyproject_toml_returns_parse_error_not_panic() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");
        fs::write(&pyproject_path, "\u{FEFF}").unwrap();

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

        // BOM-only content strips down to an empty string, which parses as an
        // empty (valid) TOML document; must not panic.
        let manifest = PyprojectToml::open(&decl, &ctx).unwrap();
        assert!(manifest.package_name().is_err());
    }

    #[test]
    fn garbage_pyproject_toml_returns_parse_error_not_panic() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");
        fs::write(&pyproject_path, "\u{FEFF}not valid toml {{{").unwrap();

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

        let result = PyprojectToml::open(&decl, &ctx);
        assert!(matches!(result, Err(ManifestError::Parse { .. })));
    }

    #[test]
    fn preserves_tab_indentation_pyproject_toml() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");

        fs::write(
            &pyproject_path,
            callisto_fixtures::corpus::pyproject_toml_tab_indented_sample(),
        )
        .unwrap();

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
        assert_eq!(manifest.package_name().unwrap(), "tabbed-lib");

        let new_v = Version::parse("1.0.1", VersionGrammar::Pep440).unwrap();
        manifest.write_version(&new_v, &permit()).unwrap();
        manifest.persist(&permit()).unwrap();

        let updated = fs::read_to_string(&pyproject_path).unwrap();
        assert!(updated.contains("\t\"requests>=2.28.0\""));
        assert!(updated.contains("version = \"1.0.1\""));
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
                &permit(),
            )
            .unwrap();

        manifest.persist(&permit()).unwrap();
        let updated_content = fs::read_to_string(&pyproject_path).unwrap();
        assert!(updated_content.contains("my-lib>=0.3.2"));
    }

    #[test]
    fn update_dependency_spec_does_not_touch_disk_until_persist_called() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");
        let content = "[project]\nname = \"my-app\"\nversion = \"0.1.0\"\ndependencies = [\n    \"my-lib>=0.3.0\",\n]\n";
        fs::write(&pyproject_path, content).unwrap();

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
                &permit(),
            )
            .unwrap();

        let unchanged = fs::read_to_string(&pyproject_path).unwrap();
        assert_eq!(
            unchanged, content,
            "update_dependency_spec alone must not write to disk"
        );

        manifest.persist(&permit()).unwrap();
        let updated = fs::read_to_string(&pyproject_path).unwrap();
        assert!(updated.contains("my-lib>=0.3.2"));
    }

    #[test]
    fn update_dependency_spec_non_range_variant_is_ok_and_persist_is_a_no_op() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");
        let content = "[project]\nname = \"my-app\"\nversion = \"0.1.0\"\ndependencies = [\n    \"my-lib>=0.3.0\",\n]\n";
        fs::write(&pyproject_path, content).unwrap();

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
        let result = manifest.update_dependency_spec(
            "my-lib",
            DepKind::Runtime,
            DepSpec::Opaque("path-dep".to_string()),
            &permit(),
        );
        assert!(
            result.is_ok(),
            "non-Range/non-Exact DepSpec must return Ok(()) without mutating"
        );

        manifest.persist(&permit()).unwrap();
        let on_disk = fs::read_to_string(&pyproject_path).unwrap();
        assert_eq!(
            on_disk, content,
            "persist of an untouched document must reproduce identical bytes"
        );
    }

    // -- round_trip tests ---------------------------------------------------

    fn make_pypi_spec(raw: &str) -> DepSpec {
        let req = VersionReq::parse(raw, Ecosystem::Pypi).unwrap();
        DepSpec::Range(req, raw.to_string())
    }

    fn make_pep440_version(s: &str) -> Version {
        Version::parse(s, VersionGrammar::Pep440).unwrap()
    }

    fn raw_of(spec: Option<DepSpec>) -> Option<String> {
        match spec {
            Some(DepSpec::Range(_, raw)) => Some(raw),
            _ => None,
        }
    }

    #[test]
    fn round_trip_exact_pin_rewrites_to_new_version() {
        let spec = make_pypi_spec("==1.2.3");
        let target = make_pep440_version("2.0.0");
        assert_eq!(
            raw_of(round_trip(&spec, &target)).as_deref(),
            Some("==2.0.0")
        );
    }

    #[test]
    fn round_trip_compatible_release_rewrites_to_new_version() {
        let spec = make_pypi_spec("~=1.4.2");
        let target = make_pep440_version("2.1.3");
        assert_eq!(
            raw_of(round_trip(&spec, &target)).as_deref(),
            Some("~=2.1.3")
        );
    }

    #[test]
    fn round_trip_lower_bound_only_rewrites_to_new_version() {
        let spec = make_pypi_spec(">=1.0.0");
        let target = make_pep440_version("2.5.1");
        assert_eq!(
            raw_of(round_trip(&spec, &target)).as_deref(),
            Some(">=2.5.1")
        );
    }

    #[test]
    fn round_trip_range_rewrites_lower_and_computes_next_major_upper() {
        let spec = make_pypi_spec(">=1.0.0,<2");
        let target = make_pep440_version("2.5.1");
        assert_eq!(
            raw_of(round_trip(&spec, &target)).as_deref(),
            Some(">=2.5.1,<3")
        );
    }

    #[test]
    fn round_trip_range_with_minor_upper_bound_computes_next_major() {
        let spec = make_pypi_spec(">=1.2.0,<2.0");
        let target = make_pep440_version("3.0.0");
        assert_eq!(
            raw_of(round_trip(&spec, &target)).as_deref(),
            Some(">=3.0.0,<4")
        );
    }

    #[test]
    fn round_trip_prerelease_target_returns_none() {
        let spec = make_pypi_spec(">=1.0.0");
        let target = make_pep440_version("2.0.0a1");
        assert!(round_trip(&spec, &target).is_none());
    }

    #[test]
    fn round_trip_upper_bound_only_returns_none() {
        let spec = make_pypi_spec("<2.0.0");
        let target = make_pep440_version("1.5.0");
        assert!(round_trip(&spec, &target).is_none());
    }

    #[test]
    fn round_trip_exclusion_returns_none() {
        let spec = make_pypi_spec("!=1.5.0");
        let target = make_pep440_version("2.0.0");
        assert!(round_trip(&spec, &target).is_none());
    }

    #[test]
    fn round_trip_non_range_spec_returns_none() {
        let target = make_pep440_version("2.0.0");
        let spec = DepSpec::Opaque("some-path-dep".to_string());
        assert!(round_trip(&spec, &target).is_none());
    }

    // -- bug regression tests -----------------------------------------------

    #[test]
    fn update_dependency_spec_returns_err_when_dep_not_found() {
        // Bug 1: if the dep name does not appear in any deps section the
        // function must return Err(DependencyNotFound), not Ok(()).
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");

        let input_content = r#"[project]
name = "my-app"
version = "0.1.0"
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
        let req = VersionReq::parse(">=1.0.0", Ecosystem::Pypi).unwrap();
        let result = manifest.update_dependency_spec(
            "nonexistent-dep",
            DepKind::Runtime,
            DepSpec::Range(req, ">=1.0.0".to_string()),
            &permit(),
        );
        assert!(
            matches!(result, Err(ManifestError::DependencyNotFound { .. })),
            "expected DependencyNotFound, got: {result:?}"
        );
    }

    #[test]
    fn update_optional_dependencies_actually_writes_to_document() {
        // Bug 2: update_optional_dependencies must iterate
        // [project.optional-dependencies] and update matching entries instead
        // of being a no-op.
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");

        let input_content = r#"[project]
name = "my-app"
version = "0.1.0"
dependencies = []

[project.optional-dependencies]
docs = ["sphinx>=4.0.0"]
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
        let new_v = Version::parse("5.0.0", VersionGrammar::Pep440).unwrap();
        manifest
            .update_optional_dependencies(&[("sphinx".to_string(), new_v)], &permit())
            .unwrap();
        manifest.persist(&permit()).unwrap();

        let updated = fs::read_to_string(&pyproject_path).unwrap();
        assert!(
            updated.contains("sphinx>=5.0.0"),
            "expected sphinx to be updated to >=5.0.0, got:\n{updated}"
        );
    }

    #[test]
    fn update_optional_dependencies_does_not_touch_disk_until_persist_called() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");
        let content = "[project]\nname = \"my-app\"\nversion = \"0.1.0\"\ndependencies = []\n\n[project.optional-dependencies]\ndocs = [\"sphinx>=4.0.0\"]\n";
        fs::write(&pyproject_path, content).unwrap();

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
        let new_v = Version::parse("5.0.0", VersionGrammar::Pep440).unwrap();
        manifest
            .update_optional_dependencies(&[("sphinx".to_string(), new_v)], &permit())
            .unwrap();

        let unchanged = fs::read_to_string(&pyproject_path).unwrap();
        assert_eq!(
            unchanged, content,
            "update_optional_dependencies alone must not write to disk"
        );

        manifest.persist(&permit()).unwrap();
        let updated = fs::read_to_string(&pyproject_path).unwrap();
        assert!(updated.contains("sphinx>=5.0.0"));
    }

    /// Spec: `PyprojectToml` must support the `[tool.poetry]` schema as a
    /// fallback when `[project]` is absent. Poetry is a popular Python build
    /// backend; workspaces that have not migrated to PEP 621 use this schema.
    /// Both `package_name()` and `current_version()` must delegate to
    /// `[tool.poetry]` when `[project]` does not exist.
    #[test]
    fn poetry_pyproject_toml_is_detected_without_project_table() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");

        let content = r#"[build-system]
requires = ["poetry-core>=1.0.0"]
build-backend = "poetry.core.masonry.api"

[tool.poetry]
name = "my-poetry-pkg"
version = "3.1.4"
description = "A Poetry-managed package"

[tool.poetry.dependencies]
python = "^3.9"
requests = "^2.28.0"
"#;
        fs::write(&pyproject_path, content).unwrap();

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
        assert_eq!(
            manifest.package_name().unwrap(),
            "my-poetry-pkg",
            "package_name must read from [tool.poetry] when [project] is absent"
        );
        assert_eq!(
            manifest.current_version().unwrap().render(),
            "3.1.4",
            "current_version must read from [tool.poetry] when [project] is absent"
        );
    }

    /// Spec: a `pyproject.toml` that has neither a `[project]` table nor a
    /// `[tool.poetry]` table must make `package_name()` return
    /// `Err(ManifestError::MissingField)`. An absent `[project]` section is a
    /// common scenario for build-system-only files (e.g. a pure
    /// `pyproject.toml` used only for packaging metadata configuration).
    #[test]
    fn pyproject_toml_without_project_or_poetry_returns_missing_field_error() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");

        // Only a build-system section, no [project] and no [tool.poetry].
        let content = r#"[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.hatch.build]
exclude = ["tests/"]
"#;
        fs::write(&pyproject_path, content).unwrap();

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
        let result = manifest.package_name();
        assert!(
            matches!(result, Err(ManifestError::MissingField { .. })),
            "missing [project] and [tool.poetry] must return MissingField, got: {result:?}"
        );
        // current_version must also return an error, not panic.
        let ver_result = manifest.current_version();
        assert!(
            ver_result.is_err(),
            "current_version without [project] or [tool.poetry] must return Err"
        );
    }

    #[test]
    fn round_trip_range_with_lte_upper_bound_returns_none() {
        // A "<=X.Y.Z" upper bound is not the recognized pattern; return None.
        let spec = make_pypi_spec(">=1.0.0,<=2.0.0");
        let target = make_pep440_version("1.5.0");
        assert!(round_trip(&spec, &target).is_none());
    }

    #[test]
    fn round_trip_returns_none_for_three_clause_spec() {
        // A three-clause spec like ">=1.0.0,<2.0.0,!=1.5.0" must not be
        // silently rewritten — the third clause would be dropped, producing a
        // semantically different constraint.  round_trip must return None and
        // leave the decision to the user.
        let spec = make_pypi_spec(">=1.0.0,<2.0.0,!=1.5.0");
        let target = make_pep440_version("1.2.0");
        assert!(round_trip(&spec, &target).is_none());
    }

    // ---- F-07: is_publishable must be false for nameless pyproject.toml ----

    fn open_pyproject(dir: &std::path::Path, content: &str) -> PyprojectToml {
        let path = dir.join("pyproject.toml");
        std::fs::write(&path, content).unwrap();
        let decl = callisto_model::ManifestDecl {
            path: std::path::PathBuf::from("pyproject.toml"),
            format: ManifestFormat::PyprojectToml,
            role: callisto_model::ManifestRole::Canonical,
        };
        let ctx = OpenContext {
            workspace_root: dir,
            cargo_workspace: None,
            npm_workspace_kind: None,
        };
        PyprojectToml::open(&decl, &ctx).unwrap()
    }

    /// A `pyproject.toml` that has no `[project].name` or `[tool.poetry].name`
    /// is a workspace root or build-system-only file — it is not publishable.
    /// Before this fix, `is_publishable` always returned `true` (trait default),
    /// causing such files to produce a `Pypi` publish target and get included
    /// in publish plans spuriously.
    #[test]
    fn pyproject_without_name_is_not_publishable() {
        let dir = tempdir().unwrap();
        let manifest = open_pyproject(
            dir.path(),
            "[build-system]\nrequires = [\"hatchling\"]\nbuild-backend = \"hatchling.build\"\n",
        );
        assert!(
            !manifest.is_publishable(),
            "pyproject.toml without [project].name must not be publishable"
        );
        assert_eq!(
            manifest.publish_targets(),
            vec![PublishTarget::None],
            "a non-publishable pyproject.toml must return PublishTarget::None"
        );
    }

    /// A `pyproject.toml` with `[project].name` IS publishable.
    #[test]
    fn pyproject_with_pep621_name_is_publishable() {
        let dir = tempdir().unwrap();
        let manifest = open_pyproject(
            dir.path(),
            "[project]\nname = \"my-pkg\"\nversion = \"1.0.0\"\n",
        );
        assert!(
            manifest.is_publishable(),
            "pyproject.toml with [project].name must be publishable"
        );
        assert_eq!(
            manifest.publish_targets(),
            vec![PublishTarget::Pypi { index: None }],
        );
    }

    /// A `pyproject.toml` with `[tool.poetry].name` IS publishable (Poetry schema).
    #[test]
    fn pyproject_with_poetry_name_is_publishable() {
        let dir = tempdir().unwrap();
        let manifest = open_pyproject(
            dir.path(),
            "[tool.poetry]\nname = \"my-pkg\"\nversion = \"1.0.0\"\n\n[tool.poetry.dependencies]\npython = \"^3.10\"\n",
        );
        assert!(
            manifest.is_publishable(),
            "pyproject.toml with [tool.poetry].name must be publishable"
        );
    }

    /// Poetry 2.x projects can have a `[project]` table (for `requires-python`,
    /// `name`, `dynamic = ["version"]`, etc.) WITHOUT a `version` field in
    /// `[project]`.  The canonical version lives in `[tool.poetry].version`.
    ///
    /// Before this fix, `write_version` dispatched on whether `[project]` exists
    /// (not whether `[project].version` exists), so it inserted a new
    /// `[project].version` key and left `[tool.poetry].version` at the old value.
    /// `current_version()` then read the newly-inserted `[project].version` and
    /// reported the write successful — but Poetry still published the old version.
    #[test]
    fn write_version_uses_poetry_section_when_project_table_has_no_version_field() {
        let dir = tempdir().unwrap();
        let content = "[project]\nname = \"my-project\"\nrequires-python = \">=3.9\"\ndynamic = [\"version\"]\n\n[tool.poetry]\nname = \"my-project\"\nversion = \"1.0.0\"\n\n[tool.poetry.dependencies]\npython = \"^3.9\"\n";
        let mut manifest = open_pyproject(dir.path(), content);

        let new_v = Version::parse("1.1.0", VersionGrammar::Pep440).unwrap();
        manifest.write_version(&new_v, &permit()).unwrap();
        manifest.persist(&permit()).unwrap();

        let on_disk = fs::read_to_string(dir.path().join("pyproject.toml")).unwrap();
        let doc: toml_edit::DocumentMut = on_disk.parse().unwrap();

        assert!(
            doc.get("project").and_then(|p| p.get("version")).is_none(),
            "[project].version must NOT be inserted when [project] has no version field;\
             got document:\n{on_disk}"
        );
        let poetry_ver = doc
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("version"))
            .and_then(|v| v.as_str())
            .expect("[tool.poetry].version must be present");
        assert_eq!(
            poetry_ver, "1.1.0",
            "[tool.poetry].version must be updated to 1.1.0; got document:\n{on_disk}"
        );
    }

    #[test]
    fn persist_is_exposed_as_a_manifest_trait_method() {
        let dir = tempdir().unwrap();
        let pyproject_path = dir.path().join("pyproject.toml");
        let content = "[project]\nname = \"my-python-lib\"\nversion = \"0.3.1\"\n";
        fs::write(&pyproject_path, content).unwrap();

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
        <PyprojectToml as Manifest>::persist(&mut manifest, &permit()).unwrap();

        let after = fs::read_to_string(&pyproject_path).unwrap();
        assert_eq!(
            after, content,
            "persist() with no prior mutation must reproduce the file unchanged"
        );
    }
}
