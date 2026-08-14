use std::fs;
use std::path::{Path, PathBuf};

use callisto_model::{
    ApplyPermit, DepKind, DepSpec, DependencyEntry, Ecosystem, ManifestDecl, ManifestError,
    ManifestFormat, ManifestRole, Version, VersionGrammar, VersionReq, WorkspaceKind,
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

fn format_json_pretty(
    map: &IndexMap<&String, &Value>,
    indent_str: &str,
) -> Result<String, ManifestError> {
    use serde::Serialize;
    let buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent_str.as_bytes());
    let mut serializer = serde_json::Serializer::with_formatter(buf, formatter);
    map.serialize(&mut serializer)
        .map_err(|e| ManifestError::FormattingNotPreserved {
            path: std::path::PathBuf::new(),
            message: format!("JSON serialization failed: {e}"),
        })?;
    let bytes = serializer.into_inner();
    let mut out = String::from_utf8(bytes).map_err(|e| ManifestError::FormattingNotPreserved {
        path: std::path::PathBuf::new(),
        message: format!("JSON serialization produced invalid UTF-8: {e}"),
    })?;
    out.push('\n');
    Ok(out)
}

impl Manifest for PackageJson {
    fn persist(&mut self, permit: &ApplyPermit) -> Result<(), ManifestError> {
        let indent_str = match self.fingerprint.indent {
            Indent::Spaces(n) => " ".repeat(n as usize),
            Indent::Tabs => "\t".to_string(),
            Indent::DefaultTwoSpaces => "  ".to_string(),
        };

        let mut map = IndexMap::new();
        for (k, v) in &self.doc {
            map.insert(k, v);
        }

        let mut out = format_json_pretty(&map, &indent_str)?;
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

        crate::atomic::atomic_write(&self.absolute, &out, permit).map_err(|e| {
            ManifestError::Write {
                path: self.path.clone(),
                message: e.to_string(),
            }
        })?;
        crate::record_persist_call();
        Ok(())
    }

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

    fn publish_targets(&self) -> Vec<callisto_model::PublishTarget> {
        if !self.is_publishable() {
            return vec![callisto_model::PublishTarget::None];
        }
        // Read `publishConfig.registry` from package.json, the standard npm
        // mechanism for targeting a private registry. When set, `npm publish`
        // must receive `--registry <url>`; when absent, the public registry is
        // used (npm's built-in default).
        let publish_config = self.doc.get("publishConfig");
        let registry = publish_config
            .and_then(|pc| pc.get("registry"))
            .and_then(|r| r.as_str())
            .map(|s| s.to_string());
        // Read `publishConfig.access` to propagate the operator's explicit
        // access intent. npm's `--access` CLI flag overrides publishConfig.access,
        // so callisto must read it here and honour it in plan_publish rather than
        // blindly passing `--access public` for all scoped packages. Any value
        // other than the two npm recognises (including absence) is `None`,
        // not an error -- an unrecognised value here isn't this layer's job
        // to validate.
        let access = publish_config
            .and_then(|pc| pc.get("access"))
            .and_then(|a| a.as_str())
            .and_then(|s| match s {
                "public" => Some(callisto_model::NpmAccess::Public),
                "restricted" => Some(callisto_model::NpmAccess::Restricted),
                _ => None,
            });
        vec![callisto_model::PublishTarget::Npm { registry, access }]
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

    fn write_version(&mut self, v: &Version, _permit: &ApplyPermit) -> Result<(), ManifestError> {
        self.doc
            .insert("version".to_string(), Value::String(v.render().to_string()));
        Ok(())
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
        _permit: &ApplyPermit,
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

        // Validate that the target dependency actually exists in the primary
        // section *before* mutating anything. This guarantees that a failed
        // update (dependency not found) leaves self.doc byte-for-byte
        // unchanged, including the overrides/resolutions tables below.
        let updated_in_section = self
            .doc
            .get(section_name)
            .and_then(|v| v.as_object())
            .is_some_and(|section| section.contains_key(name));

        if !updated_in_section {
            return Err(ManifestError::DependencyNotFound {
                path: self.path.clone(),
                name: name.to_string(),
                kind,
            });
        }

        if let Some(section) = self
            .doc
            .get_mut(section_name)
            .and_then(|v| v.as_object_mut())
        {
            section.insert(name.to_string(), Value::String(new.render()));
        }

        for extra in ["overrides", "resolutions"] {
            if let Some(tbl) = self.doc.get_mut(extra).and_then(|v| v.as_object_mut()) {
                if tbl.get(name).is_some_and(|v| v.is_string()) {
                    tbl.insert(name.to_string(), Value::String(new.render()));
                }
            }
        }

        Ok(())
    }

    fn update_optional_dependencies(
        &mut self,
        updates: &[(String, Version)],
        _permit: &ApplyPermit,
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

        Ok(())
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

    if crate::common::is_bare_semver(s) {
        if let Ok(v) = Version::parse(s, VersionGrammar::SemVer) {
            return DepSpec::Exact(v);
        }
    }

    if let Ok(req) = VersionReq::parse(s, Ecosystem::Npm) {
        return DepSpec::Range(req, s.to_string());
    }

    DepSpec::Opaque(s.to_string())
}

pub fn round_trip(spec: &DepSpec, target: &Version) -> Option<DepSpec> {
    match spec {
        DepSpec::Exact(_) => Some(DepSpec::Exact(target.clone())),
        DepSpec::Range(_, original) => {
            let (prefix, rest) = crate::common::split_single_operator_prefix(original)?;
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

fn render_at_precision(target: &Version, original_clause: &str) -> String {
    if target.is_prerelease() {
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

    /// Tests exercise the write primitives directly rather than through a
    /// command handler, so they mint a permit without a dry-run flag to
    /// consult. Every non-test caller must go through
    /// `ApplyPermit::granted_unless_dry_run`.
    fn permit() -> callisto_model::ApplyPermit {
        callisto_model::ApplyPermit::force_for_tests()
    }
    use super::*;

    /// Spec: `format_json_pretty` must propagate errors via Result rather
    /// than unwrapping. The return type must be `Result<String, _>` so
    /// callers can handle failures without a panic.
    ///
    /// Before the fix the return type is `String`, so the `is_ok()` call
    /// below fails to compile (String has no `is_ok` method).
    #[test]
    fn format_json_pretty_returns_result_not_panics() {
        let k = "version".to_string();
        let v = Value::String("1.0.0".to_string());
        let mut map = IndexMap::new();
        map.insert(&k, &v);
        let result: Result<String, _> = format_json_pretty(&map, "  ");
        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.contains("\"version\""));
        assert!(json.contains("\"1.0.0\""));
    }
    use callisto_model::{ManifestDecl, ManifestFormat, ManifestRole};
    use tempfile::tempdir;

    /// Helper: write `content` to `<dir>/package.json` and open it as a
    /// `PackageJson`. The caller must keep `dir` alive for the duration of the
    /// test so that the tempdir is not cleaned up prematurely.
    fn open_manifest(dir: &tempfile::TempDir, content: &str) -> PackageJson {
        let manifest_path = dir.path().join("package.json");
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
        PackageJson::open(&decl, &ctx).unwrap()
    }

    #[test]
    fn write_version_does_not_touch_disk_until_persist_called() {
        let dir = tempdir().unwrap();
        let content = "{\n  \"name\": \"@myorg/pkg\",\n  \"version\": \"1.0.0\"\n}\n";
        let manifest_path = dir.path().join("package.json");
        let mut manifest = open_manifest(&dir, content);

        let new_ver = Version::parse("1.1.0", VersionGrammar::SemVer).unwrap();
        manifest.write_version(&new_ver, &permit()).unwrap();

        let unchanged = fs::read_to_string(&manifest_path).unwrap();
        assert_eq!(
            unchanged, content,
            "write_version alone must not write to disk"
        );

        manifest.persist(&permit()).unwrap();
        let updated = fs::read_to_string(&manifest_path).unwrap();
        assert!(updated.contains("\"version\": \"1.1.0\""));
    }

    #[test]
    fn update_optional_dependencies_does_not_touch_disk_until_persist_called() {
        let dir = tempdir().unwrap();
        let content = "{\n  \"name\": \"@myorg/pkg\",\n  \"version\": \"1.0.0\",\n  \"optionalDependencies\": {\n    \"fsevents\": \"1.0.0\"\n  }\n}\n";
        let manifest_path = dir.path().join("package.json");
        let mut manifest = open_manifest(&dir, content);

        let new_v = Version::parse("2.0.0", VersionGrammar::SemVer).unwrap();
        manifest
            .update_optional_dependencies(&[("fsevents".to_string(), new_v)], &permit())
            .unwrap();

        let unchanged = fs::read_to_string(&manifest_path).unwrap();
        assert_eq!(
            unchanged, content,
            "update_optional_dependencies alone must not write to disk"
        );

        manifest.persist(&permit()).unwrap();
        let updated = fs::read_to_string(&manifest_path).unwrap();
        assert!(updated.contains("\"fsevents\": \"2.0.0\""));
    }

    #[test]
    fn update_dependency_spec_does_not_touch_disk_until_persist_called() {
        let dir = tempdir().unwrap();
        let content = "{\n  \"name\": \"@myorg/pkg\",\n  \"version\": \"1.0.0\",\n  \"dependencies\": {\n    \"lodash\": \"^4.17.0\"\n  }\n}\n";
        let manifest_path = dir.path().join("package.json");
        let mut manifest = open_manifest(&dir, content);

        manifest
            .update_dependency_spec(
                "lodash",
                DepKind::Runtime,
                DepSpec::Opaque("^5.0.0".to_string()),
                &permit(),
            )
            .unwrap();

        let unchanged = fs::read_to_string(&manifest_path).unwrap();
        assert_eq!(
            unchanged, content,
            "update_dependency_spec alone must not write to disk"
        );

        manifest.persist(&permit()).unwrap();
        let updated = fs::read_to_string(&manifest_path).unwrap();
        assert!(updated.contains("\"lodash\": \"^5.0.0\""));
    }

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
        manifest.write_version(&new_ver, &permit()).unwrap();
        manifest.persist(&permit()).unwrap();

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

    #[test]
    fn update_dependency_spec_does_not_mutate_overrides_when_primary_section_missing() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("package.json");
        let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "express": "^4.18.0"
  },
  "overrides": {
    "lodash": "^4.17.0"
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

        let before = manifest.doc.get("overrides").cloned();

        // "lodash" exists in overrides but NOT in the "dependencies" section,
        // so the primary-section update must fail.
        let result = manifest.update_dependency_spec(
            "lodash",
            DepKind::Runtime,
            DepSpec::Opaque("^5.0.0".to_string()),
            &permit(),
        );

        assert!(result.is_err());

        let after = manifest.doc.get("overrides").cloned();
        assert_eq!(
            before, after,
            "overrides table must remain unchanged when the primary section update fails"
        );

        // The file on disk must also remain untouched.
        let on_disk = fs::read_to_string(&manifest_path).unwrap();
        assert!(on_disk.contains("\"lodash\": \"^4.17.0\""));
    }

    // --- Gap 1: happy-path co-mutation of overrides/resolutions ---

    #[test]
    fn update_dependency_spec_co_mutates_overrides() {
        let dir = tempdir().unwrap();
        let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "lodash": "^4.17.0"
  },
  "overrides": {
    "lodash": "^4.17.0"
  }
}
"#;
        let mut manifest = open_manifest(&dir, content);

        manifest
            .update_dependency_spec(
                "lodash",
                DepKind::Runtime,
                DepSpec::Opaque("^5.0.0".to_string()),
                &permit(),
            )
            .unwrap();

        let deps = manifest
            .doc
            .get("dependencies")
            .and_then(|v| v.as_object())
            .unwrap();
        assert_eq!(
            deps.get("lodash").and_then(|v| v.as_str()),
            Some("^5.0.0"),
            "dependencies must reflect the new version"
        );

        let overrides = manifest
            .doc
            .get("overrides")
            .and_then(|v| v.as_object())
            .unwrap();
        assert_eq!(
            overrides.get("lodash").and_then(|v| v.as_str()),
            Some("^5.0.0"),
            "overrides must be co-mutated when the dep exists in the primary section"
        );
    }

    #[test]
    fn update_dependency_spec_co_mutates_resolutions() {
        let dir = tempdir().unwrap();
        let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "lodash": "^4.17.0"
  },
  "resolutions": {
    "lodash": "^4.17.0"
  }
}
"#;
        let mut manifest = open_manifest(&dir, content);

        manifest
            .update_dependency_spec(
                "lodash",
                DepKind::Runtime,
                DepSpec::Opaque("^5.0.0".to_string()),
                &permit(),
            )
            .unwrap();

        let deps = manifest
            .doc
            .get("dependencies")
            .and_then(|v| v.as_object())
            .unwrap();
        assert_eq!(
            deps.get("lodash").and_then(|v| v.as_str()),
            Some("^5.0.0"),
            "dependencies must reflect the new version"
        );

        let resolutions = manifest
            .doc
            .get("resolutions")
            .and_then(|v| v.as_object())
            .unwrap();
        assert_eq!(
            resolutions.get("lodash").and_then(|v| v.as_str()),
            Some("^5.0.0"),
            "resolutions must be co-mutated when the dep exists in the primary section"
        );
    }

    #[test]
    fn update_dependency_spec_co_mutates_overrides_and_resolutions() {
        let dir = tempdir().unwrap();
        let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "lodash": "^4.17.0"
  },
  "overrides": {
    "lodash": "^4.17.0"
  },
  "resolutions": {
    "lodash": "^4.17.0"
  }
}
"#;
        let mut manifest = open_manifest(&dir, content);

        manifest
            .update_dependency_spec(
                "lodash",
                DepKind::Runtime,
                DepSpec::Opaque("^5.0.0".to_string()),
                &permit(),
            )
            .unwrap();

        let deps = manifest
            .doc
            .get("dependencies")
            .and_then(|v| v.as_object())
            .unwrap();
        assert_eq!(
            deps.get("lodash").and_then(|v| v.as_str()),
            Some("^5.0.0"),
            "dependencies must reflect the new version"
        );

        let overrides = manifest
            .doc
            .get("overrides")
            .and_then(|v| v.as_object())
            .unwrap();
        assert_eq!(
            overrides.get("lodash").and_then(|v| v.as_str()),
            Some("^5.0.0"),
            "overrides must be co-mutated"
        );

        let resolutions = manifest
            .doc
            .get("resolutions")
            .and_then(|v| v.as_object())
            .unwrap();
        assert_eq!(
            resolutions.get("lodash").and_then(|v| v.as_str()),
            Some("^5.0.0"),
            "resolutions must be co-mutated"
        );
    }

    /// Co-mutation is keyed by name: updating "express" must not touch the
    /// "lodash" entry that happens to live in overrides.
    #[test]
    fn update_dependency_spec_co_mutation_is_name_scoped() {
        let dir = tempdir().unwrap();
        let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "express": "^4.18.0"
  },
  "overrides": {
    "lodash": "^4.17.0"
  }
}
"#;
        let mut manifest = open_manifest(&dir, content);

        manifest
            .update_dependency_spec(
                "express",
                DepKind::Runtime,
                DepSpec::Opaque("^5.0.0".to_string()),
                &permit(),
            )
            .unwrap();

        let deps = manifest
            .doc
            .get("dependencies")
            .and_then(|v| v.as_object())
            .unwrap();
        assert_eq!(
            deps.get("express").and_then(|v| v.as_str()),
            Some("^5.0.0"),
            "express in dependencies must be updated"
        );

        let overrides = manifest
            .doc
            .get("overrides")
            .and_then(|v| v.as_object())
            .unwrap();
        assert_eq!(
            overrides.get("lodash").and_then(|v| v.as_str()),
            Some("^4.17.0"),
            "lodash in overrides must be untouched (different name)"
        );
    }

    // --- Gap 2: resolutions missing from validate-before-mutate regression ---

    #[test]
    fn update_dependency_spec_does_not_mutate_resolutions_when_primary_section_missing() {
        let dir = tempdir().unwrap();
        let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "express": "^4.18.0"
  },
  "resolutions": {
    "lodash": "^4.17.0"
  }
}
"#;
        let mut manifest = open_manifest(&dir, content);

        let before = manifest.doc.get("resolutions").cloned();

        let result = manifest.update_dependency_spec(
            "lodash",
            DepKind::Runtime,
            DepSpec::Opaque("^5.0.0".to_string()),
            &permit(),
        );

        assert!(
            result.is_err(),
            "update must fail when dep is absent from primary section"
        );

        let after = manifest.doc.get("resolutions").cloned();
        assert_eq!(
            before, after,
            "resolutions table must remain unchanged when the primary section update fails"
        );
    }

    /// Combined case: package in both overrides AND resolutions but not in
    /// the primary section. Both tables must be untouched on failure.
    #[test]
    fn update_dependency_spec_does_not_mutate_overrides_or_resolutions_when_primary_section_missing(
    ) {
        let dir = tempdir().unwrap();
        let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "express": "^4.18.0"
  },
  "overrides": {
    "lodash": "^4.17.0"
  },
  "resolutions": {
    "lodash": "^4.17.0"
  }
}
"#;
        let mut manifest = open_manifest(&dir, content);

        let before_overrides = manifest.doc.get("overrides").cloned();
        let before_resolutions = manifest.doc.get("resolutions").cloned();

        let result = manifest.update_dependency_spec(
            "lodash",
            DepKind::Runtime,
            DepSpec::Opaque("^5.0.0".to_string()),
            &permit(),
        );

        assert!(
            result.is_err(),
            "update must fail when dep is absent from primary section"
        );

        let after_overrides = manifest.doc.get("overrides").cloned();
        let after_resolutions = manifest.doc.get("resolutions").cloned();

        assert_eq!(
            before_overrides, after_overrides,
            "overrides table must remain unchanged when the primary section update fails"
        );
        assert_eq!(
            before_resolutions, after_resolutions,
            "resolutions table must remain unchanged when the primary section update fails"
        );
    }

    // --- Gap 4: nested overrides object must not be replaced with a string ---

    /// When `overrides["foo"]` is a nested object (e.g. `{ "bar": "^1.0.0" }`),
    /// bumping `foo` must leave that object intact. The nested form expresses a
    /// package-level constraint that is not a simple version pin and must not be
    /// overwritten.
    #[test]
    fn update_dependency_spec_does_not_overwrite_nested_overrides_object() {
        let dir = tempdir().unwrap();
        let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "foo": "^1.0.0"
  },
  "overrides": {
    "foo": {
      "bar": "^1.0.0"
    }
  }
}
"#;
        let mut manifest = open_manifest(&dir, content);

        manifest
            .update_dependency_spec(
                "foo",
                DepKind::Runtime,
                DepSpec::Opaque("^2.0.0".to_string()),
                &permit(),
            )
            .unwrap();

        let overrides_foo = manifest
            .doc
            .get("overrides")
            .and_then(|v| v.as_object())
            .and_then(|o| o.get("foo"))
            .expect("overrides.foo must still exist");

        assert!(
            overrides_foo.is_object(),
            "overrides.foo must remain an object, but got: {overrides_foo:?}"
        );
    }

    // --- Gap 3: DepKind::Dev validate-before-mutate path ---

    #[test]
    fn update_dependency_spec_does_not_mutate_overrides_when_dev_dep_missing() {
        let dir = tempdir().unwrap();
        let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "devDependencies": {
    "typescript": "^5.0.0"
  },
  "overrides": {
    "lodash": "^4.17.0"
  }
}
"#;
        let mut manifest = open_manifest(&dir, content);

        let before = manifest.doc.get("overrides").cloned();

        let result = manifest.update_dependency_spec(
            "lodash",
            DepKind::Dev,
            DepSpec::Opaque("^5.0.0".to_string()),
            &permit(),
        );

        assert!(
            result.is_err(),
            "update must fail when dep is absent from devDependencies"
        );

        let after = manifest.doc.get("overrides").cloned();
        assert_eq!(
            before, after,
            "overrides table must remain unchanged when the dev dep update fails"
        );
    }

    /// A package.json that contains a top-level `"workspaces"` array must be
    /// detected as an NPM workspace root by `detect_npm_workspace_kind`, even
    /// when no lock-file is present on disk (the lock-file heuristic alone is
    /// not sufficient for projects that have not yet run `npm install`).
    #[test]
    fn detects_workspace_kind_from_workspaces_field_in_package_json() {
        let dir = tempdir().unwrap();
        let pkg_json = dir.path().join("package.json");
        let content = r#"{
  "name": "my-monorepo",
  "version": "1.0.0",
  "workspaces": [
    "packages/*"
  ]
}
"#;
        fs::write(&pkg_json, content).unwrap();

        // No lock-file on disk, only the "workspaces" field in package.json.
        let kind = detect_npm_workspace_kind(dir.path()).unwrap();
        assert_eq!(
            kind,
            Some(WorkspaceKind::Npm),
            "workspaces field in package.json must imply WorkspaceKind::Npm"
        );
    }

    /// A scoped package name like `"@scope/name"` must round-trip through
    /// `package_name()` unchanged. Scoped names contain a `/` character that
    /// could be mistaken for a path separator; this test verifies it is
    /// treated as a literal package-name character.
    #[test]
    fn scoped_package_name_is_parsed_correctly() {
        let dir = tempdir().unwrap();
        let content = r#"{
  "name": "@my-scope/awesome-pkg",
  "version": "2.3.4"
}
"#;
        let manifest = open_manifest(&dir, content);
        assert_eq!(
            manifest.package_name().unwrap(),
            "@my-scope/awesome-pkg",
            "scoped package name must be returned verbatim"
        );
    }

    /// A `package.json` that is missing the `"version"` field must make
    /// `current_version()` return `Err(ManifestError::MissingField)` rather
    /// than panicking or returning a default value.
    #[test]
    fn missing_version_field_returns_missing_field_error_not_panic() {
        let dir = tempdir().unwrap();
        let content = r#"{
  "name": "no-version-pkg",
  "description": "This package intentionally omits the version field"
}
"#;
        let manifest = open_manifest(&dir, content);
        let result = manifest.current_version();
        assert!(
            matches!(
                result,
                Err(ManifestError::MissingField {
                    field: "version",
                    ..
                })
            ),
            "missing version field must return MissingField error, got: {result:?}"
        );
    }

    /// Tab-indented package.json files must have their indentation preserved after a
    /// version write. This catches regressions where the CST editor re-serializes with
    /// two-space indentation instead of the original tab characters.
    #[test]
    fn tab_indentation_is_preserved_after_version_write() {
        use callisto_model::WorkspaceKind;
        let dir = tempdir().unwrap();
        let path = dir.path().join("package.json");
        let content = "{\n\t\"name\": \"tab-app\",\n\t\"version\": \"1.0.0\"\n}\n";
        fs::write(&path, content).unwrap();

        let decl = ManifestDecl::new(
            "package.json",
            ManifestRole::Canonical,
            ManifestFormat::PackageJson,
        )
        .unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            npm_workspace_kind: Some(WorkspaceKind::Pnpm),
        };

        let mut pj = PackageJson::open(&decl, &ctx).unwrap();
        pj.write_version(
            &callisto_model::Version::parse("1.1.0", callisto_model::VersionGrammar::SemVer)
                .unwrap(),
            &permit(),
        )
        .unwrap();
        pj.persist(&permit()).unwrap();

        let updated = fs::read_to_string(&path).unwrap();
        assert!(updated.contains("\"version\": \"1.1.0\""));
        assert!(
            updated.contains("\t\"version\""),
            "tab indentation must be preserved after version write"
        );
    }

    // --- round_trip tests ---------------------------------------------------

    fn make_npm_spec(raw: &str) -> DepSpec {
        let req = VersionReq::parse(raw, Ecosystem::Npm).unwrap();
        DepSpec::Range(req, raw.to_string())
    }

    fn npm_version(s: &str) -> Version {
        Version::parse(s, VersionGrammar::SemVer).unwrap()
    }

    fn raw_of(spec: Option<DepSpec>) -> Option<String> {
        match spec {
            Some(DepSpec::Range(_, raw)) => Some(raw),
            _ => None,
        }
    }

    #[test]
    fn round_trip_exact_spec_rewrites_to_target_version() {
        let spec = DepSpec::Exact(npm_version("1.0.0"));
        let target = npm_version("2.3.4");
        match round_trip(&spec, &target) {
            Some(DepSpec::Exact(v)) => assert_eq!(v.render(), "2.3.4"),
            other => panic!("expected Some(Exact(2.3.4)), got {other:?}"),
        }
    }

    #[test]
    fn round_trip_caret_range_rewrites_to_target_version() {
        let spec = make_npm_spec("^1.0.0");
        let target = npm_version("1.5.2");
        assert_eq!(
            raw_of(round_trip(&spec, &target)).as_deref(),
            Some("^1.5.2")
        );
    }

    #[test]
    fn round_trip_tilde_range_rewrites_to_target_version() {
        let spec = make_npm_spec("~1.0.0");
        let target = npm_version("1.0.9");
        assert_eq!(
            raw_of(round_trip(&spec, &target)).as_deref(),
            Some("~1.0.9")
        );
    }

    #[test]
    fn round_trip_gte_range_rewrites_to_target_version() {
        let spec = make_npm_spec(">=1.0.0");
        let target = npm_version("2.5.1");
        assert_eq!(
            raw_of(round_trip(&spec, &target)).as_deref(),
            Some(">=2.5.1")
        );
    }

    #[test]
    fn round_trip_exact_operator_range_rewrites_to_target_version() {
        let spec = make_npm_spec("=1.0.0");
        let target = npm_version("1.2.3");
        assert_eq!(
            raw_of(round_trip(&spec, &target)).as_deref(),
            Some("=1.2.3")
        );
    }

    #[test]
    fn round_trip_preserves_two_part_precision() {
        let spec = make_npm_spec("^1.0");
        let target = npm_version("1.5.2");
        assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some("^1.5"));
    }

    #[test]
    fn round_trip_preserves_one_part_precision() {
        let spec = make_npm_spec("^1");
        let target = npm_version("2.5.2");
        assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some("^2"));
    }

    /// A prerelease target version (e.g. publishing `2.0.0-beta.1`) must be
    /// rendered in full, not truncated to `major.minor.patch` -- dropping the
    /// prerelease suffix would produce a range that does not actually match
    /// the version it claims to pin to (`^2.0.0` excludes `2.0.0-beta.1`
    /// under semver precedence rules).
    #[test]
    fn round_trip_prerelease_target_is_rendered_in_full() {
        let spec = make_npm_spec("^1.0.0");
        let target = npm_version("2.0.0-beta.1");
        assert_eq!(
            raw_of(round_trip(&spec, &target)).as_deref(),
            Some("^2.0.0-beta.1")
        );
    }

    #[test]
    fn round_trip_non_range_variants_return_none() {
        let target = npm_version("2.0.0");
        assert!(round_trip(&DepSpec::Opaque("weird".to_string()), &target).is_none());
        assert!(round_trip(&DepSpec::Workspace(WorkspaceKind::Npm), &target).is_none());
        assert!(round_trip(&DepSpec::Catalog(None), &target).is_none());
        assert!(round_trip(&DepSpec::CargoBare(npm_version("1.0.0")), &target).is_none());
    }

    /// A clause with a prerelease suffix (e.g. `^1.0.0-beta.1`) is declined
    /// rather than rewritten -- `render_at_precision` only knows how to
    /// truncate a plain `major.minor.patch` clause, so a prerelease-qualified
    /// original clause safely falls back to `RewriteOutcome::LeftAlone`.
    #[test]
    fn round_trip_prerelease_clause_returns_none() {
        let spec = make_npm_spec("^1.0.0-beta.1");
        let target = npm_version("2.0.0");
        assert!(round_trip(&spec, &target).is_none());
    }

    /// A wildcard clause (`1.x`, `1.X`, `^1.x`) is declined rather than
    /// rewritten -- these are not simple `{operator}{version}` clauses and
    /// `render_at_precision` has no wildcard-preserving behavior.
    #[test]
    fn round_trip_wildcard_clause_returns_none() {
        let target = npm_version("2.5.0");
        assert!(round_trip(&make_npm_spec("1.x"), &target).is_none());
        assert!(round_trip(&make_npm_spec("1.X"), &target).is_none());
        assert!(round_trip(&make_npm_spec("^1.x"), &target).is_none());
    }

    /// A comma-separated compound range (`>=1.0.0, <2.0.0`) parses
    /// successfully -- npm and Cargo share the same underlying semver
    /// requirement grammar in this codebase (`semver::VersionReq`), so this
    /// atypical-for-npm-but-syntactically-valid form does reach this
    /// function -- but it is declined rather than rewritten. Unlike
    /// `cargo::round_trip`, `npm::round_trip` has no per-clause compound-range
    /// handling, so this documents current, intentional (safe-fallback)
    /// behavior: the caller falls back to `RewriteOutcome::LeftAlone` with a
    /// diagnostic instead of guessing at a rewrite.
    #[test]
    fn round_trip_comma_compound_range_returns_none() {
        let spec = make_npm_spec(">=1.0.0, <2.0.0");
        let target = npm_version("1.5.0");
        assert!(round_trip(&spec, &target).is_none());
    }

    #[test]
    fn persist_is_exposed_as_a_manifest_trait_method() {
        let dir = tempdir().unwrap();
        let content = "{\n  \"name\": \"@myorg/pkg\",\n  \"version\": \"1.0.0\"\n}\n";
        let manifest_path = dir.path().join("package.json");
        let mut manifest = open_manifest(&dir, content);

        <PackageJson as Manifest>::persist(&mut manifest, &permit()).unwrap();

        let after = fs::read_to_string(&manifest_path).unwrap();
        assert_eq!(
            after, content,
            "persist() with no prior mutation must reproduce the file unchanged"
        );
    }
}
