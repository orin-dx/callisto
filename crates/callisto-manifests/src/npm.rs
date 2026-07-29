use std::fs;
use std::path::{Path, PathBuf};

use callisto_model::{
    DepKind, DepSpec, DependencyEntry, Ecosystem, ManifestDecl, ManifestError, ManifestFormat,
    ManifestRole, Version, VersionGrammar, VersionReq, WorkspaceKind,
};
use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::{Manifest, OpenContext};

pub struct PackageJson {
    path: PathBuf,
    absolute: PathBuf,
    role: ManifestRole,
    doc: Map<String, Value>,
    fingerprint: FormatFingerprint,
    npm_workspace_kind: Option<WorkspaceKind>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FormatFingerprint {
    indent: Indent,
    trailing_newline: bool,
    line_ending: LineEnding,
    has_bom: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Indent {
    Spaces(u8),
    Tabs,
    DefaultTwoSpaces,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineEnding {
    Lf,
    CrLf,
}

impl PackageJson {
    pub fn open(decl: &ManifestDecl, ctx: &OpenContext<'_>) -> Result<Self, ManifestError> {
        let rel_path = decl.path.clone();
        let abs_path = ctx.workspace_root.join(&rel_path);

        let content = fs::read_to_string(&abs_path).map_err(|e| ManifestError::Read {
            path: rel_path.clone(),
            message: e.to_string(),
        })?;

        let has_bom = content.starts_with('\u{FEFF}');
        let clean_content = content.strip_prefix('\u{FEFF}').unwrap_or(&content);
        let mut fingerprint = detect_fingerprint(clean_content);
        fingerprint.has_bom = has_bom;

        let doc: Map<String, Value> =
            serde_json::from_str(clean_content).map_err(|e| ManifestError::Parse {
                path: rel_path.clone(),
                format: ManifestFormat::PackageJson,
                message: e.to_string(),
            })?;

        Ok(PackageJson {
            path: rel_path,
            absolute: abs_path,
            role: decl.role.clone(),
            doc,
            fingerprint,
            npm_workspace_kind: ctx.npm_workspace_kind,
        })
    }

    fn persist(&mut self) -> Result<(), ManifestError> {
        let indent_str = match self.fingerprint.indent {
            Indent::Spaces(n) => " ".repeat(n as usize),
            Indent::Tabs => "\t".to_string(),
            Indent::DefaultTwoSpaces => "  ".to_string(),
        };

        let mut map = IndexMap::new();
        for (k, v) in &self.doc {
            map.insert(k, v);
        }

        let mut out = format_json_pretty(&map, &indent_str);
        if self.fingerprint.line_ending == LineEnding::CrLf {
            out = out.replace("\r\n", "\n").replace('\n', "\r\n");
        }

        if !self.fingerprint.trailing_newline && out.ends_with('\n') {
            if out.ends_with("\r\n") {
                out.truncate(out.len() - 2);
            } else {
                out.truncate(out.len() - 1);
            }
        }

        if self.fingerprint.has_bom {
            out = format!("\u{FEFF}{}", out);
        }

        crate::atomic::atomic_write(&self.absolute, &out).map_err(|e| ManifestError::Write {
            path: self.path.clone(),
            message: e.to_string(),
        })
    }
}

fn detect_fingerprint(content: &str) -> FormatFingerprint {
    let line_ending = if content.contains("\r\n") {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    };

    let trailing_newline = content.ends_with('\n');

    let mut indent = Indent::DefaultTwoSpaces;
    for line in content.lines() {
        let trimmed_start = line.trim_start();
        if trimmed_start.starts_with('"') && line.len() > trimmed_start.len() {
            let leading = &line[..line.len() - trimmed_start.len()];
            if leading.contains('\t') {
                indent = Indent::Tabs;
            } else {
                indent = Indent::Spaces(leading.len() as u8);
            }
            break;
        }
    }

    let has_bom = content.starts_with('\u{FEFF}');
    FormatFingerprint {
        indent,
        trailing_newline,
        line_ending,
        has_bom,
    }
}

fn format_json_pretty(map: &IndexMap<&String, &Value>, indent_str: &str) -> String {
    use serde::Serialize;
    let buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent_str.as_bytes());
    let mut serializer = serde_json::Serializer::with_formatter(buf, formatter);
    map.serialize(&mut serializer).unwrap();
    let mut out = String::from_utf8(serializer.into_inner()).unwrap();
    out.push('\n');
    out
}

impl Manifest for PackageJson {
    fn path(&self) -> &Path {
        &self.path
    }

    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Npm
    }

    fn is_publishable(&self) -> bool {
        if self.doc.get("name").is_none() {
            return false;
        }
        if let Some(b) = self.doc.get("private").and_then(|p| p.as_bool()) {
            if b {
                return false;
            }
        }
        true
    }

    fn role(&self) -> ManifestRole {
        self.role.clone()
    }

    fn package_name(&self) -> Result<String, ManifestError> {
        let name = self
            .doc
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| ManifestError::MissingField {
                path: self.path.clone(),
                field: "name",
            })?;
        Ok(name.to_string())
    }

    fn current_version(&self) -> Result<Version, ManifestError> {
        let raw = self
            .doc
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ManifestError::MissingField {
                path: self.path.clone(),
                field: "version",
            })?;

        Version::parse(raw, VersionGrammar::SemVer).map_err(|source| {
            ManifestError::InvalidVersion {
                path: self.path.clone(),
                raw: raw.to_string(),
                source,
            }
        })
    }

    fn write_version(&mut self, v: &Version) -> Result<(), ManifestError> {
        self.doc
            .insert("version".to_string(), Value::String(v.render().to_string()));
        self.persist()
    }

    fn iter_dependencies(&self) -> Box<dyn Iterator<Item = DependencyEntry> + '_> {
        let mut entries = Vec::new();

        for (section_name, kind) in [
            ("dependencies", DepKind::Runtime),
            ("devDependencies", DepKind::Dev),
            ("peerDependencies", DepKind::Peer),
            ("optionalDependencies", DepKind::Optional),
        ] {
            if let Some(obj) = self.doc.get(section_name).and_then(|v| v.as_object()) {
                for (name, val) in obj {
                    if let Some(v_str) = val.as_str() {
                        let spec = parse_npm_spec_str(v_str, self.npm_workspace_kind);
                        entries.push(DependencyEntry {
                            name: name.clone(),
                            kind,
                            spec,
                            inherited: false,
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
    ) -> Result<(), ManifestError> {
        if kind == DepKind::Build {
            return Err(ManifestError::DependencyNotFound {
                path: self.path.clone(),
                name: name.to_string(),
                kind,
            });
        }

        let section_name = match kind {
            DepKind::Runtime => "dependencies",
            DepKind::Dev => "devDependencies",
            DepKind::Peer => "peerDependencies",
            DepKind::Optional => "optionalDependencies",
            DepKind::Build => unreachable!(),
        };

        let updated_in_section = if let Some(section) = self
            .doc
            .get_mut(section_name)
            .and_then(|v| v.as_object_mut())
        {
            if section.contains_key(name) {
                section.insert(name.to_string(), Value::String(new.render()));
                true
            } else {
                false
            }
        } else {
            false
        };

        for extra in ["overrides", "resolutions"] {
            if let Some(tbl) = self.doc.get_mut(extra).and_then(|v| v.as_object_mut()) {
                if tbl.contains_key(name) {
                    tbl.insert(name.to_string(), Value::String(new.render()));
                }
            }
        }

        if !updated_in_section {
            return Err(ManifestError::DependencyNotFound {
                path: self.path.clone(),
                name: name.to_string(),
                kind,
            });
        }

        self.persist()
    }

    fn update_optional_dependencies(
        &mut self,
        updates: &[(String, Version)],
    ) -> Result<(), ManifestError> {
        let section = self
            .doc
            .entry("optionalDependencies".to_string())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| ManifestError::FormattingNotPreserved {
                path: self.path.clone(),
                message: "optionalDependencies is not an object".to_string(),
            })?;

        for (name, ver) in updates {
            section.insert(name.clone(), Value::String(ver.render().to_string()));
        }

        self.persist()
    }
}

fn parse_npm_spec_str(s: &str, ws_kind: Option<WorkspaceKind>) -> DepSpec {
    if let Some(rest) = s.strip_prefix("workspace:") {
        if let Some(kind) = ws_kind {
            return DepSpec::Workspace(kind);
        }
        return DepSpec::Opaque(format!("workspace:{rest}"));
    }
    if let Some(rest) = s.strip_prefix("catalog:") {
        let name = if rest.is_empty() {
            None
        } else {
            Some(rest.to_string())
        };
        return DepSpec::Catalog(name);
    }

    if is_bare_semver(s) {
        if let Ok(v) = Version::parse(s, VersionGrammar::SemVer) {
            return DepSpec::Exact(v);
        }
    }

    if let Ok(req) = VersionReq::parse(s, Ecosystem::Npm) {
        return DepSpec::Range(req, s.to_string());
    }

    DepSpec::Opaque(s.to_string())
}

fn is_bare_semver(s: &str) -> bool {
    let chars = s.chars().next();
    if !chars.is_some_and(|c| c.is_ascii_digit()) {
        return false;
    }
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3
}

pub fn round_trip(spec: &DepSpec, target: &Version) -> Option<DepSpec> {
    match spec {
        DepSpec::Exact(_) => Some(DepSpec::Exact(target.clone())),
        DepSpec::Range(_, original) => {
            let (prefix, rest) = split_single_operator_prefix(original)?;
            if rest.contains(' ')
                || rest.contains('-')
                || rest.contains('|')
                || rest.contains(['x', 'X', '*'])
            {
                return None;
            }
            let rendered = format!("{prefix}{}", render_at_precision(target, rest));
            let req = VersionReq::parse(&rendered, Ecosystem::Npm).ok()?;
            Some(DepSpec::Range(req, rendered))
        }
        _ => None,
    }
}

fn split_single_operator_prefix(s: &str) -> Option<(&str, &str)> {
    let trimmed = s.trim();
    for op in ["^", "~", ">=", ">", "<=", "<", "="] {
        if let Some(rest) = trimmed.strip_prefix(op) {
            return Some((op, rest.trim()));
        }
    }
    Some(("", trimmed))
}

fn render_at_precision(target: &Version, original_clause: &str) -> String {
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

pub fn detect_npm_workspace_kind(
    workspace_root: &Path,
) -> Result<Option<WorkspaceKind>, ManifestError> {
    if workspace_root.join("pnpm-lock.yaml").exists() {
        return Ok(Some(WorkspaceKind::Pnpm));
    }
    if workspace_root.join("yarn.lock").exists() {
        return Ok(Some(WorkspaceKind::Yarn));
    }
    if workspace_root.join("package-lock.json").exists() {
        return Ok(Some(WorkspaceKind::Npm));
    }

    let root_pkg_json = workspace_root.join("package.json");
    if root_pkg_json.exists() {
        if let Ok(content) = fs::read_to_string(&root_pkg_json) {
            if let Ok(val) = serde_json::from_str::<Value>(&content) {
                if val.get("workspaces").is_some() {
                    return Ok(Some(WorkspaceKind::Npm));
                }
            }
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::{ManifestDecl, ManifestFormat, ManifestRole};
    use tempfile::tempdir;

    #[test]
    fn parses_package_json_and_updates_version() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("package.json");
        let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "express": "^4.18.0"
  }
}
"#;
        fs::write(&manifest_path, content).unwrap();

        let decl = ManifestDecl::new(
            "package.json",
            ManifestRole::Canonical,
            ManifestFormat::PackageJson,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let mut manifest = PackageJson::open(&decl, &ctx).unwrap();
        assert_eq!(manifest.package_name().unwrap(), "@myorg/pkg");
        assert_eq!(manifest.current_version().unwrap().render(), "1.0.0");

        let new_ver = Version::parse("1.1.0", VersionGrammar::SemVer).unwrap();
        manifest.write_version(&new_ver).unwrap();

        let updated_content = fs::read_to_string(&manifest_path).unwrap();
        assert!(updated_content.contains("\"version\": \"1.1.0\""));
    }

    #[test]
    fn detects_workspace_kind() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(
            detect_npm_workspace_kind(dir.path()).unwrap(),
            Some(WorkspaceKind::Pnpm)
        );
    }

    #[test]
    fn parses_package_json_with_utf8_bom() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("package.json");
        let content = "\u{FEFF}{\n  \"name\": \"bom-pkg\",\n  \"version\": \"1.0.0\"\n}\n";
        fs::write(&manifest_path, content).unwrap();

        let decl = ManifestDecl::new(
            "package.json",
            ManifestRole::Canonical,
            ManifestFormat::PackageJson,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: None,
        };

        let manifest = PackageJson::open(&decl, &ctx).unwrap();
        assert_eq!(manifest.package_name().unwrap(), "bom-pkg");
        assert_eq!(manifest.current_version().unwrap().render(), "1.0.0");
    }
}
