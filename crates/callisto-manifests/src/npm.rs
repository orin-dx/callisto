use std::fs;
use std::path::{Path, PathBuf};

use callisto_model::{
    ApplyPermit, DepKind, DepLocator, DepSpec, DependencyEntry, Ecosystem, ManifestDecl, ManifestError, ManifestFormat,
    ManifestRole, Version, VersionGrammar, VersionReq, WorkspaceKind, WorkspaceSpec,
};
use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::{Manifest, NpmRole, OpenContext};

pub struct PackageJson {
    path: PathBuf,
    absolute: PathBuf,
    role: ManifestRole,
    doc: Map<String, Value>,
    fingerprint: FormatFingerprint,
    npm_workspace_kind: Option<WorkspaceKind>,
    workspace_root: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FormatFingerprint {
    indent: Indent,
    trailing_newline: bool,
    common: crate::common::FormatFingerprint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Indent {
    Spaces(u8),
    Tabs,
    DefaultTwoSpaces,
}

impl PackageJson {
    pub fn open(decl: &ManifestDecl, ctx: &OpenContext<'_>) -> Result<Self, ManifestError> {
        let rel_path = decl.path.clone();
        let abs_path = ctx.workspace_root.join(&rel_path);

        let content = fs::read_to_string(&abs_path).map_err(|e| ManifestError::Read {
            path: rel_path.clone(),
            message: e.to_string(),
        })?;

        let common_fp = crate::common::FormatFingerprint::detect(&content);
        let clean_content = content.strip_prefix('\u{FEFF}').unwrap_or(&content);
        let fingerprint = detect_fingerprint(clean_content, common_fp);

        let doc: Map<String, Value> = serde_json::from_str(clean_content).map_err(|e| ManifestError::Parse {
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
            workspace_root: ctx.workspace_root.to_path_buf(),
        })
    }

    fn dependency_target(&self, name: &str, spec: &DepSpec) -> String {
        let DepSpec::WorkspaceProtocol(workspace) = spec else {
            return name.to_string();
        };
        if let Some(alias) = workspace.alias() {
            return alias.to_string();
        }
        let Some(relative) = workspace.relative_path() else {
            return name.to_string();
        };
        let target_name = self.absolute.parent().and_then(|parent| {
            let target = parent.join(relative).canonicalize().ok()?;
            let root = self.workspace_root.canonicalize().ok()?;
            if !target.starts_with(root) {
                return None;
            }
            let manifest = target.join("package.json");
            let source = fs::read_to_string(&manifest).ok()?;
            identity_from_source(&source, &manifest).ok()?.name
        });
        target_name.unwrap_or_else(|| format!("workspace:{}", workspace.suffix))
    }
}

fn detect_fingerprint(content: &str, common: crate::common::FormatFingerprint) -> FormatFingerprint {
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

    FormatFingerprint {
        indent,
        trailing_newline,
        common,
    }
}

fn format_json_pretty(map: &IndexMap<&String, &Value>, indent_str: &str) -> Result<String, ManifestError> {
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

/// Extracts `name` from an already-parsed `package.json` document. Pure and
/// I/O-free -- see [`crate::cargo::cargo_package_name`]'s doc comment for
/// why this exists as a standalone function rather than only living inside
/// [`Manifest::package_name`].
pub fn npm_package_name(doc: &serde_json::Map<String, serde_json::Value>) -> Option<&str> {
    doc.get("name").and_then(|n| n.as_str())
}

/// [`crate::read_identity`]'s `package.json` implementation: parses
/// `source` and extracts `name` plus a [`crate::VersionSource::Literal`]
/// for `version` (npm has no workspace-inherited-version concept, unlike
/// Cargo).
pub(crate) fn identity_from_source(source: &str, path: &Path) -> Result<crate::ManifestIdentity, ManifestError> {
    let clean = source.strip_prefix('\u{FEFF}').unwrap_or(source);
    let doc: Map<String, Value> = serde_json::from_str(clean).map_err(|e| ManifestError::Parse {
        path: path.to_path_buf(),
        format: ManifestFormat::PackageJson,
        message: e.to_string(),
    })?;

    let name = npm_package_name(&doc).map(str::to_string);
    let version = doc
        .get("version")
        .and_then(|v| v.as_str())
        .map(|s| crate::VersionSource::Literal(s.to_string()));

    Ok(crate::ManifestIdentity { name, version })
}

/// Reads `napi.targets` from an already-parsed `package.json` value.
/// `Ok(None)` when `napi` or `napi.targets` is absent -- not every napi
/// package.json declares platform targets. `Err` when `napi.targets` is
/// present but is not a JSON array of strings.
///
/// The single implementation both `NapiTargetsIndex::load` (lenient: `.ok()`
/// this, treating a malformed value the same as absent) and
/// `matrix::read_napi_targets` (strict: propagates the error) build their
/// differing policies on top of -- see each call site for which it chooses.
pub fn read_napi_targets(path: &Path, val: &Value) -> Result<Option<Vec<String>>, ManifestError> {
    let Some(napi) = val.get("napi") else {
        return Ok(None);
    };
    let Some(targets) = napi.get("targets") else {
        return Ok(None);
    };

    let arr = targets.as_array().ok_or_else(|| ManifestError::Parse {
        path: path.to_path_buf(),
        format: ManifestFormat::PackageJson,
        message: "napi.targets must be a JSON array of strings".to_string(),
    })?;

    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item.as_str().ok_or_else(|| ManifestError::Parse {
            path: path.to_path_buf(),
            format: ManifestFormat::PackageJson,
            message: "napi.targets entries must all be strings".to_string(),
        })?;
        out.push(s.to_string());
    }
    Ok(Some(out))
}

/// The napi-rs binary name: `napi.binaryName`, else the deprecated `napi.name`, else napi-rs's `index`.
pub fn napi_binary_name(val: &Value) -> String {
    let napi = val.get("napi");
    ["binaryName", "name"]
        .iter()
        .find_map(|key| napi.and_then(|napi| napi.get(key)).and_then(Value::as_str))
        .unwrap_or("index")
        .to_owned()
}

/// Detects whether `doc` (an already-parsed `package.json` document) declares
/// itself a napi-rs platform package -- `os`+`cpu` constraint arrays both
/// present and non-empty -- and if so, its platform/arch/abi. Pure and
/// I/O-free: the single implementation [`PackageJson::npm_role`] builds on,
/// so platform-role detection is available from an already-open `Manifest`
/// handle (e.g. one served from `callisto-graph`'s `manifest_cache`) without
/// a second `fs::read` of the same file.
/// `npm_role_from_doc` over raw `package.json` source, for callers (workspace
/// discovery) that have not opened a manifest handle. `None` on invalid JSON.
pub fn npm_role_from_source(source: &str) -> Option<NpmRole> {
    match serde_json::from_str::<Value>(source).ok()? {
        Value::Object(doc) => npm_role_from_doc(&doc),
        _ => None,
    }
}

/// True when raw `package.json` `source` sets `"private": true`. `false` on
/// invalid JSON or an absent/non-true `private` field.
pub fn npm_declares_private(source: &str) -> bool {
    match serde_json::from_str::<Value>(source).ok() {
        Some(Value::Object(doc)) => doc.get("private").and_then(Value::as_bool).unwrap_or(false),
        _ => false,
    }
}

fn npm_role_from_doc(doc: &Map<String, Value>) -> Option<NpmRole> {
    let has_os = doc.get("os").and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty());
    let has_cpu = doc.get("cpu").and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty());

    if !has_os || !has_cpu {
        return None;
    }

    let platform = doc
        .get("os")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .map(str::to_string)?;

    let arch = doc
        .get("cpu")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .map(str::to_string)?;

    // npm's standard `os`/`cpu` manifest fields have no libc/ABI concept at
    // all, so a real disk-discovered Linux napi platform package always
    // produces `abi: None` here -- but `napi.rs::role_to_triple`'s Linux
    // match arms all require a concrete ABI, meaning such a package could
    // never resolve to its triple. napi-rs's own package-generation
    // convention encodes the ABI in the package *name*'s suffix instead
    // (e.g. "@scope/pkg-linux-x64-gnu"), so infer it from there when the
    // platform is linux.
    let abi = if platform == "linux" {
        doc.get("name")
            .and_then(|v| v.as_str())
            .and_then(napi_linux_abi_from_package_name)
    } else {
        None
    };

    Some(NpmRole::Platform { platform, arch, abi })
}

/// Infers a Linux napi-rs platform package's libc ABI from its package
/// name's trailing suffix. Returns `None` when the name has no recognized
/// suffix -- this infers when the signal is present, it doesn't invent an
/// ABI that isn't actually there.
fn napi_linux_abi_from_package_name(name: &str) -> Option<String> {
    for abi in ["gnueabihf", "gnu", "musl"] {
        if name.ends_with(&format!("-{abi}")) {
            return Some(abi.to_string());
        }
    }
    None
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
        if self.fingerprint.common.line_ending == crate::common::LineEnding::CrLf {
            out = out.replace("\r\n", "\n").replace('\n', "\r\n");
        }

        // Trailing-newline trimming must happen strictly between the
        // line-ending conversion (above, so a trimmed "\r\n" is measured
        // correctly) and the BOM prepend (below, so a BOM byte is never
        // mistaken for part of the trailing newline) -- this ordering
        // dependency is why npm's own trailing_newline concern can't route
        // through `common::FormatFingerprint::apply`'s single combined
        // crlf-then-bom step the way cargo/python's simpler persist paths do.
        if !self.fingerprint.trailing_newline && out.ends_with('\n') {
            if out.ends_with("\r\n") {
                out.truncate(out.len() - 2);
            } else {
                out.truncate(out.len() - 1);
            }
        }

        if self.fingerprint.common.has_bom {
            out = format!("\u{FEFF}{}", out);
        }

        crate::atomic::atomic_write(&self.absolute, &out, permit).map_err(|e| ManifestError::Write {
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
        Ecosystem::Npm
    }

    fn is_publishable(&self) -> bool {
        if self.doc.get("name").is_none() {
            return false;
        }
        if let Some(b) = self.doc.get("private").and_then(|p| p.as_bool())
            && b
        {
            return false;
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
        // so callisto must read it here and honour it in release planning rather than
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
        let name = npm_package_name(&self.doc).ok_or_else(|| ManifestError::MissingField {
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

        Version::parse(raw, VersionGrammar::SemVer).map_err(|source| ManifestError::InvalidVersion {
            path: self.path.clone(),
            raw: raw.to_string(),
            source,
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
                            package_name: self.dependency_target(name, &spec),
                            kind,
                            spec,
                            inherited: false,
                            locator: DepLocator::default(),
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
        _at: &DepLocator,
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

        if let Some(section) = self.doc.get_mut(section_name).and_then(|v| v.as_object_mut()) {
            section.insert(name.to_string(), Value::String(new.render()));
        }

        if matches!(new, DepSpec::WorkspaceProtocol(_)) {
            return Ok(());
        }

        for extra in ["overrides", "resolutions"] {
            if let Some(tbl) = self.doc.get_mut(extra).and_then(|v| v.as_object_mut())
                && tbl.get(name).is_some_and(|v| v.is_string())
            {
                tbl.insert(name.to_string(), Value::String(new.render()));
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

    fn npm_role(&self) -> Option<NpmRole> {
        npm_role_from_doc(&self.doc)
    }

    fn bin_names(&self) -> Result<Vec<String>, ManifestError> {
        Ok(match self.doc.get("bin") {
            // npm installs a string `bin` under the unscoped package name.
            Some(Value::String(_)) => npm_package_name(&self.doc)
                .map(|name| name.rsplit('/').next().unwrap_or(name).to_string())
                .into_iter()
                .collect(),
            Some(Value::Object(bins)) => bins.keys().cloned().collect(),
            _ => Vec::new(),
        })
    }
}

fn parse_npm_spec_str(s: &str, ws_kind: Option<WorkspaceKind>) -> DepSpec {
    if let Some(rest) = s.strip_prefix("workspace:") {
        if let Some(kind) = ws_kind {
            return DepSpec::WorkspaceProtocol(WorkspaceSpec {
                kind,
                suffix: rest.to_string(),
            });
        }
        return DepSpec::Opaque(format!("workspace:{rest}"));
    }
    if let Some(rest) = s.strip_prefix("catalog:") {
        let name = if rest.is_empty() { None } else { Some(rest.to_string()) };
        return DepSpec::Catalog(name);
    }

    if crate::common::is_bare_semver(s)
        && let Ok(v) = Version::parse(s, VersionGrammar::SemVer)
    {
        return DepSpec::Exact(v);
    }

    if let Ok(req) = VersionReq::parse(s, Ecosystem::Npm) {
        return DepSpec::Range(req, s.to_string());
    }

    DepSpec::Opaque(s.to_string())
}

pub fn round_trip(spec: &DepSpec, target: &Version) -> Option<DepSpec> {
    match spec {
        DepSpec::WorkspaceProtocol(workspace) => {
            if workspace.coverage(target).ok()? != callisto_model::Coverage::DoesNotCover {
                return None;
            }
            let range = workspace.range()?;
            let rewritten = if Version::parse(range, VersionGrammar::SemVer).is_ok() {
                target.render().to_string()
            } else {
                let req = VersionReq::parse(range, Ecosystem::Npm).ok()?;
                round_trip(&DepSpec::Range(req, range.to_string()), target)?.render()
            };
            let updated = workspace.with_range(&rewritten);
            if updated.coverage(target).ok()? != callisto_model::Coverage::Covers {
                return None;
            }
            Some(DepSpec::WorkspaceProtocol(updated))
        }
        DepSpec::Exact(_) => Some(DepSpec::Exact(target.clone())),
        DepSpec::Range(_, original) => {
            let (prefix, rest) = crate::common::split_single_operator_prefix(original)?;
            // A prerelease clause (`^2.0.0-next.0`) is one version; a hyphen range or a set of ranges is not.
            let core = rest.split(['-', '+']).next().unwrap_or(rest);
            if rest.contains([' ', '|']) || core.contains(['x', 'X', '*']) {
                return None;
            }
            if core != rest && Version::parse(rest, VersionGrammar::SemVer).is_err() {
                return None;
            }
            let rendered = format!("{prefix}{}", render_at_precision(target, core));
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

pub fn detect_npm_workspace_kind(workspace_root: &Path) -> Result<Option<WorkspaceKind>, ManifestError> {
    let present = |name: &str| -> Result<bool, ManifestError> {
        let path = workspace_root.join(name);
        callisto_model::strict_fs::exists(&path).map_err(|e| ManifestError::Read {
            path,
            message: e.to_string(),
        })
    };
    if present("pnpm-lock.yaml")? || present("pnpm-workspace.yaml")? {
        return Ok(Some(WorkspaceKind::Pnpm));
    }
    if present("yarn.lock")? {
        return Ok(Some(WorkspaceKind::Yarn));
    }
    if present("package-lock.json")? {
        return Ok(Some(WorkspaceKind::Npm));
    }

    let root_pkg_json = workspace_root.join("package.json");
    let content = callisto_model::strict_fs::read_optional(&root_pkg_json).map_err(|e| ManifestError::Read {
        path: root_pkg_json.clone(),
        message: e.to_string(),
    })?;
    if let Some(content) = content {
        let val = serde_json::from_str::<Value>(content.strip_prefix('\u{FEFF}').unwrap_or(&content)).map_err(|e| {
            ManifestError::Parse {
                path: root_pkg_json.clone(),
                format: ManifestFormat::PackageJson,
                message: e.to_string(),
            }
        })?;
        if val.get("workspaces").is_some() {
            return Ok(Some(WorkspaceKind::Npm));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests;
