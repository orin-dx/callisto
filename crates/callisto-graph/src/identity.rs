use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use callisto_model::{Ecosystem, ManifestRole, PackageId};

use crate::error::GraphError;

/// Test-observability counter: total number of times [`IdentityResolver::resolve`]
/// has actually read and parsed a manifest from disk (i.e. missed its memo).
/// Production code never reads this; it exists so callers can write
/// regression tests asserting that a given `(path, ecosystem)` pair is
/// resolved (read + parsed) at most once per [`IdentityResolver`] lifetime,
/// instead of once per caller. Mirrors `callisto_manifests::open_call_count`.
static IDENTITY_READ_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Resets the internal identity-read call counter to zero. Intended for use in test setup.
pub fn reset_identity_read_count() {
    IDENTITY_READ_COUNT.store(0, std::sync::atomic::Ordering::SeqCst);
}

/// Reads the current value of the internal identity-read call counter.
pub fn identity_read_count() -> usize {
    IDENTITY_READ_COUNT.load(std::sync::atomic::Ordering::SeqCst)
}

pub struct IdentityResolver {
    workspace_root: PathBuf,
    /// Memoizes `resolve`'s result per `(project_root, ecosystem)` so a
    /// manifest referenced by many dependency edges (e.g. a widely-depended-on
    /// shared crate) is read and parsed from disk at most once per resolver
    /// lifetime, rather than once per caller. `MoonProjectLocator` holds a
    /// single `IdentityResolver` for its whole lifetime, so this memo spans
    /// both `projects()` and `declared_edges()` -- the two call sites that
    /// previously re-resolved the same identity independently.
    memo: Mutex<BTreeMap<(PathBuf, Ecosystem), PackageId>>,
}

impl IdentityResolver {
    pub fn new(workspace_root: &Path) -> Result<Self, GraphError> {
        Ok(IdentityResolver {
            workspace_root: workspace_root.to_path_buf(),
            memo: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn resolve(&self, project_root: &Path, ecosystem: Ecosystem) -> Result<PackageId, GraphError> {
        let key = (project_root.to_path_buf(), ecosystem);
        if let Some(id) = self.memo.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return Ok(id.clone());
        }

        let abs = self.workspace_root.join(project_root);
        let Some(format) = ecosystem.canonical_manifest_format() else {
            return Err(GraphError::AmbiguousName {
                name: "unsupported ecosystem".to_string(),
                candidates: Vec::new(),
            });
        };

        let manifest_rel = project_root.join(format.file_name());
        let manifest_abs = abs.join(format.file_name());
        IDENTITY_READ_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let content = std::fs::read_to_string(&manifest_abs).map_err(|e| callisto_model::ManifestError::Read {
            path: manifest_rel.clone(),
            message: e.to_string(),
        })?;
        let identity = callisto_manifests::read_identity(format, &content, &manifest_rel)?;
        let name = identity.name.ok_or(callisto_model::ManifestError::MissingField {
            path: manifest_rel,
            field: match ecosystem {
                Ecosystem::Cargo => "package.name",
                Ecosystem::Npm => "name",
                _ => "project.name / tool.poetry.name / tool.flit.metadata.module",
            },
        })?;

        let id = PackageId::parse(&name).map_err(|_err| GraphError::AmbiguousName {
            name: name.clone(),
            candidates: Vec::new(),
        })?;

        self.memo
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, id.clone());
        Ok(id)
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
        let id = resolver.resolve(std::path::Path::new("."), Ecosystem::Cargo).unwrap();
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
        let id = resolver.resolve(std::path::Path::new("."), Ecosystem::Cargo).unwrap();
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
        let id = resolver.resolve(std::path::Path::new("."), Ecosystem::Npm).unwrap();
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
        let id = resolver.resolve(std::path::Path::new("."), Ecosystem::Pypi).unwrap();
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
        let id = resolver.resolve(std::path::Path::new("."), Ecosystem::Pypi).unwrap();
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
        let id = resolver.resolve(std::path::Path::new("."), Ecosystem::Pypi).unwrap();
        assert_eq!(id.name(), "my_flit_lib");
    }

    #[test]
    fn resolve_errors_when_manifest_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = IdentityResolver::new(dir.path()).unwrap();
        let result = resolver.resolve(std::path::Path::new("."), Ecosystem::Cargo);
        assert!(result.is_err());
    }

    /// F16 regression: a second `resolve` call for the identical
    /// `(project_root, ecosystem)` pair must be served from the memo, not
    /// re-read from disk. Proven by deleting the manifest file between the
    /// two calls -- if the second call fell through to disk, it would error
    /// (file missing) instead of returning the memoized id.
    #[test]
    fn resolve_memoizes_and_does_not_re_read_deleted_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("Cargo.toml");
        std::fs::write(
            &manifest_path,
            "[package]\nname = \"memo-crate\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let resolver = IdentityResolver::new(dir.path()).unwrap();

        let first = resolver.resolve(std::path::Path::new("."), Ecosystem::Cargo).unwrap();
        assert_eq!(first.name(), "memo-crate");

        std::fs::remove_file(&manifest_path).unwrap();

        let second = resolver
            .resolve(std::path::Path::new("."), Ecosystem::Cargo)
            .expect("second resolve must be served from the memo, not re-read the (now-deleted) manifest");
        assert_eq!(second, first);
    }

    /// F16 regression: distinct `(path, ecosystem)` keys must not collide in
    /// the memo -- resolving one path/ecosystem pair must not poison the
    /// cache entry for a different pair.
    #[test]
    fn resolve_memo_is_keyed_by_both_path_and_ecosystem() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::create_dir_all(dir.path().join("b")).unwrap();
        std::fs::write(
            dir.path().join("a/Cargo.toml"),
            "[package]\nname = \"crate-a\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("b/package.json"),
            r#"{"name":"pkg-b","version":"1.0.0"}"#,
        )
        .unwrap();
        let resolver = IdentityResolver::new(dir.path()).unwrap();

        let a = resolver.resolve(std::path::Path::new("a"), Ecosystem::Cargo).unwrap();
        let b = resolver.resolve(std::path::Path::new("b"), Ecosystem::Npm).unwrap();
        assert_eq!(a.name(), "crate-a");
        assert_eq!(b.name(), "pkg-b");
    }

    #[test]
    fn resolve_errors_when_name_field_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        let resolver = IdentityResolver::new(dir.path()).unwrap();
        let result = resolver.resolve(std::path::Path::new("."), Ecosystem::Cargo);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_human_finds_prefixed_entry_for_unpromoted_package() {
        let mut index = IdentityIndex::default();
        let id = PackageId::Bare("foo".to_string());
        index.bare.insert("foo".to_string(), id.clone());
        index.native.insert((Ecosystem::Cargo, "foo".to_string()), id.clone());
        index.prefixed.insert((Ecosystem::Cargo, "foo".to_string()), id.clone());
        let resolved = index
            .resolve_human("cargo:foo", &[])
            .expect("cargo:foo must resolve via prefixed map");
        assert_eq!(resolved, id);
    }

    #[test]
    fn resolve_human_unknown_ecosystem_prefix_falls_through_to_unknown() {
        let mut index = IdentityIndex::default();
        let id = PackageId::Bare("foo".to_string());
        index.bare.insert("foo".to_string(), id.clone());
        index.native.insert((Ecosystem::Cargo, "foo".to_string()), id.clone());
        index.prefixed.insert((Ecosystem::Cargo, "foo".to_string()), id);
        let err = index.resolve_human("npm:foo", &[]).unwrap_err();
        assert!(
            matches!(err, GraphError::UnknownPackage { .. }),
            "expected UnknownPackage, got {err:?}"
        );
    }

    #[test]
    fn resolve_native_with_fallback_returns_none_on_single_cross_ecosystem_candidate() {
        let mut index = IdentityIndex::default();
        let cargo_serde = PackageId::Bare("serde".to_string());
        index
            .native
            .insert((Ecosystem::Cargo, "serde".to_string()), cargo_serde);
        let mut diagnostics = Vec::new();
        let result = index.resolve_native_with_fallback(Ecosystem::Npm, "serde", &mut diagnostics);
        assert!(
            result.is_none(),
            "a same-ecosystem miss must never silently fall back to a cross-ecosystem match"
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, callisto_model::DiagnosticCode::UnknownPackage);
        assert_eq!(diagnostics[0].severity, callisto_model::DiagnosticSeverity::Warning);
    }

    #[test]
    fn resolve_native_with_fallback_returns_none_and_one_diagnostic_for_two_candidates() {
        let mut index = IdentityIndex::default();
        index.native.insert(
            (Ecosystem::Cargo, "ambiguous-lib".to_string()),
            PackageId::Bare("ambiguous-lib".to_string()),
        );
        index.native.insert(
            (Ecosystem::Pypi, "ambiguous-lib".to_string()),
            PackageId::Prefixed {
                ecosystem: Ecosystem::Pypi,
                name: "ambiguous-lib".to_string(),
            },
        );
        let mut diagnostics = Vec::new();
        let result = index.resolve_native_with_fallback(Ecosystem::Npm, "ambiguous-lib", &mut diagnostics);
        assert!(result.is_none());
        assert_eq!(diagnostics.len(), 1, "exactly one diagnostic, not zero and not two");
        assert!(
            diagnostics[0].message.contains("cargo:ambiguous-lib")
                && diagnostics[0].message.contains("pypi:ambiguous-lib")
        );
    }

    #[test]
    fn resolve_native_unchanged_still_returns_cross_ecosystem_match() {
        let mut index = IdentityIndex::default();
        let cargo_serde = PackageId::Bare("serde".to_string());
        index
            .native
            .insert((Ecosystem::Cargo, "serde".to_string()), cargo_serde.clone());
        let result = index.resolve_native(Ecosystem::Npm, "serde");
        assert_eq!(
            result,
            Some(&cargo_serde),
            "resolve_native's own permissive contract is unchanged by this spec"
        );
    }
}

#[derive(Clone, Debug, Default)]
pub struct IdentityIndex {
    pub bare: BTreeMap<String, PackageId>,
    pub prefixed: BTreeMap<(Ecosystem, String), PackageId>,
    pub native: BTreeMap<(Ecosystem, String), PackageId>,
    /// Keyed by a platform npm manifest's own package name (e.g.
    /// `"@myorg/my-crate-linux-x64-gnu"`) -- distinct from `owner`'s name
    /// when the owning package's primary identity comes from a
    /// higher-priority ecosystem sharing the same directory (a Cargo+npm
    /// Case D project, §M.6.1 M7: platform manifests belong to the owning
    /// `Package`, they are never independently-registered `Package`s of
    /// their own). The `ManifestRole::Platform` is carried alongside so
    /// `GroupTable::resolve` doesn't need a second disk read to recover it.
    pub platform: BTreeMap<String, (PackageId, PathBuf, ManifestRole)>,
}

impl IdentityIndex {
    pub fn resolve_human(&self, name: &str, siblings: &[PackageId]) -> Result<PackageId, GraphError> {
        if let Ok(PackageId::Prefixed { ecosystem, name: n }) = PackageId::parse(name) {
            if let Some(id) = self.prefixed.get(&(ecosystem, n)) {
                return Ok(id.clone());
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
    /// First tries the exact `(eco, name)` key. If that misses -- a
    /// package in one ecosystem depending on a different ecosystem's
    /// package by bare name (e.g. an npm package listing a cargo crate)
    /// -- falls back to scanning all ecosystems for `name`.
    ///
    /// Fallback finds exactly one match: returns it. Finds more than one
    /// (two ecosystems both have `name`): returns `None` to prevent
    /// silent misresolution -- callers holding a `&mut Vec<Diagnostic>`
    /// should use [`Self::resolve_native_with_fallback`] instead, to get
    /// a diagnostic.
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
            _ => {
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
        for (name, (plat_owner, path, _role)) in &self.platform {
            if plat_owner == owner {
                results.push((name.as_str(), path.as_path()));
            }
        }
        results.into_iter()
    }
}
