//! Per-ecosystem manifest read/write for callisto.

use std::path::Path;
use std::sync::Arc;

use callisto_model::{
    ApplyPermit, DepSpec, Ecosystem, ManifestDecl, ManifestError, ManifestFormat, ManifestRole, Version, WorkspaceKind,
};

pub mod atomic;

pub mod cargo;
mod common;
pub mod npm;
pub mod python;

pub use cargo::{cargo_package_name, CargoToml, InheritedDep, WorkspaceCargoResolver, WorkspaceInheritance};
pub use npm::{detect_npm_workspace_kind, npm_package_name, read_napi_targets, PackageJson};
pub use python::{python_package_name, PyprojectToml, Requirement};

/// Package identity extracted directly from manifest source text via
/// [`read_identity`], without going through the full `Manifest::open`
/// lifecycle (workspace-inheritance context, on-disk path resolution,
/// atomic-write machinery). For callers that already hold a manifest's
/// content as a string -- a `git show` blob, a walker's pre-read buffer --
/// and only need the package's name and/or where its version comes from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManifestIdentity {
    pub name: Option<String>,
    pub version: Option<VersionSource>,
}

/// Where a manifest's version declaration comes from. Distinct from a raw
/// `Option<String>` so Cargo's `version.workspace = true` is a real,
/// distinguishable case instead of being silently collapsed into "no
/// version present". [`read_identity`] never resolves the inherited value
/// (it has no workspace context) -- callers needing the resolved version
/// go through `Manifest::current_version` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VersionSource {
    Literal(String),
    InheritedFromWorkspace,
}

/// Extracts a package's name and version source directly from `source`,
/// the raw text of a manifest matching `format`. Pure and I/O-free.
///
/// Replaces hand-parsing `Cargo.toml`/`package.json`/`pyproject.toml` at
/// call sites that already hold manifest content as a string rather than a
/// path `open()` can read from disk (audit pattern B) -- e.g. a `git show`
/// blob, or a directory-walker's pre-read buffer.
///
/// `path` is used only to build accurate `ManifestError::Parse` locations
/// on a parse failure; it need not exist on disk. Only the three
/// canonical-identity formats ([`Ecosystem::CANONICAL`]) are supported --
/// any other format returns `Err(ManifestError::ReadOnlyFormat)`.
pub fn read_identity(format: ManifestFormat, source: &str, path: &Path) -> Result<ManifestIdentity, ManifestError> {
    match format {
        ManifestFormat::CargoToml => cargo::identity_from_source(source, path),
        ManifestFormat::PackageJson => npm::identity_from_source(source, path),
        ManifestFormat::PyprojectToml => python::identity_from_source(source, path),
        other => Err(ManifestError::ReadOnlyFormat {
            path: path.to_path_buf(),
            format: other,
            reason: "read_identity only supports the three canonical-identity manifest formats \
                     (Cargo.toml, package.json, pyproject.toml)",
        }),
    }
}

/// Trait implemented by per-ecosystem manifest editors.
pub trait Manifest: Send + Sync {
    fn path(&self) -> &Path;
    fn ecosystem(&self) -> Ecosystem;
    fn role(&self) -> ManifestRole;
    fn package_name(&self) -> Result<String, ManifestError>;
    fn current_version(&self) -> Result<Version, ManifestError>;
    fn write_version(&mut self, v: &Version, permit: &ApplyPermit) -> Result<(), ManifestError>;
    fn persist(&mut self, permit: &ApplyPermit) -> Result<(), ManifestError>;
    fn iter_dependencies(&self) -> Box<dyn Iterator<Item = callisto_model::DependencyEntry> + '_>;
    fn update_dependency_spec(
        &mut self,
        name: &str,
        kind: callisto_model::DepKind,
        new: DepSpec,
        permit: &ApplyPermit,
    ) -> Result<(), ManifestError>;
    fn is_publishable(&self) -> bool {
        true
    }
    fn publish_targets(&self) -> Vec<callisto_model::PublishTarget> {
        if !self.is_publishable() {
            return vec![callisto_model::PublishTarget::None];
        }
        match self.ecosystem() {
            Ecosystem::Cargo => vec![callisto_model::PublishTarget::CratesIo],
            Ecosystem::Npm => vec![callisto_model::PublishTarget::Npm {
                registry: None,
                access: None,
            }],
            _ => vec![callisto_model::PublishTarget::None],
        }
    }
    fn update_optional_dependencies(
        &mut self,
        updates: &[(String, Version)],
        permit: &ApplyPermit,
    ) -> Result<(), ManifestError>;
}

/// Context passed to open() to supply workspace-wide inheritance facts.
pub struct OpenContext<'a> {
    pub workspace_root: &'a Path,
    pub cargo_workspace: Option<Arc<WorkspaceInheritance>>,
    pub npm_workspace_kind: Option<WorkspaceKind>,
}

/// Test-observability counter: total number of times [`open`] has been
/// invoked. Production code never reads this; it exists so callers (in
/// particular, callers building a caching layer on top of `open()`) can
/// write regression tests asserting that a given manifest path is opened
/// (read + parsed from disk) at most once per logical operation, instead of
/// once per call site. See `callisto-graph`'s `manifest_cache` module.
static OPEN_CALL_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Resets the internal manifest-open call counter to zero. Intended for use in test setup.
pub fn reset_open_call_count() {
    OPEN_CALL_COUNT.store(0, std::sync::atomic::Ordering::SeqCst);
}

/// Reads the current value of the internal manifest-open call counter.
pub fn open_call_count() -> usize {
    OPEN_CALL_COUNT.load(std::sync::atomic::Ordering::SeqCst)
}

/// Test-observability counter: total number of times a `Manifest`
/// implementor's `persist` has returned `Ok`. Production code never reads
/// this; it exists so callers can write regression tests asserting a given
/// manifest path is flushed to disk at most once per logical operation.
/// Distinct from `OPEN_CALL_COUNT` so open-count and persist-count
/// assertions in the same test are independent.
static PERSIST_CALL_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Resets the internal manifest-persist call counter to zero. Intended for use in test setup.
pub fn reset_persist_call_count() {
    PERSIST_CALL_COUNT.store(0, std::sync::atomic::Ordering::SeqCst);
}

/// Reads the current value of the internal manifest-persist call counter.
pub fn persist_call_count() -> usize {
    PERSIST_CALL_COUNT.load(std::sync::atomic::Ordering::SeqCst)
}

/// Increments PERSIST_CALL_COUNT by one. Called by each Manifest
/// implementor's persist() immediately after its own atomic_write returns
/// Ok; PERSIST_CALL_COUNT itself is private to this module, so this is the
/// only way other modules in this crate may increment it.
pub(crate) fn record_persist_call() {
    PERSIST_CALL_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// Opens a manifest file matching `decl.format`.
pub fn open(decl: &ManifestDecl, ctx: &OpenContext<'_>) -> Result<Box<dyn Manifest>, ManifestError> {
    OPEN_CALL_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if decl.role == ManifestRole::Lockfile {
        return Err(ManifestError::ReadOnlyFormat {
            path: decl.path.clone(),
            format: decl.format,
            reason: "lockfiles are regenerated via subprocess (§7.6 step 9), never opened as a Manifest handle",
        });
    }

    match decl.format {
        #[cfg(feature = "cargo")]
        ManifestFormat::CargoToml => Ok(Box::new(CargoToml::open(decl, ctx)?)),
        #[cfg(feature = "npm")]
        ManifestFormat::PackageJson => Ok(Box::new(PackageJson::open(decl, ctx)?)),
        #[cfg(feature = "pypi")]
        ManifestFormat::PyprojectToml => Ok(Box::new(PyprojectToml::open(decl, ctx)?)),
        other => Err(ManifestError::ReadOnlyFormat {
            path: decl.path.clone(),
            format: other,
            reason: "not implemented — demand-gated per §2.2",
        }),
    }
}

/// Dispatches spec round-trip rewriting to the appropriate ecosystem handler.
pub fn round_trip(ecosystem: Ecosystem, spec: &DepSpec, target: &Version) -> Option<DepSpec> {
    match (ecosystem, spec) {
        #[cfg(feature = "cargo")]
        (Ecosystem::Cargo, _) => cargo::round_trip(spec, target),
        #[cfg(feature = "npm")]
        (Ecosystem::Npm, _) => npm::round_trip(spec, target),
        #[cfg(feature = "pypi")]
        (Ecosystem::Pypi, _) => python::round_trip(spec, target),
        _ => None,
    }
}

#[cfg(all(test, feature = "pypi"))]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn open_dispatches_pyproject_toml_to_python_manifest() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"demo-pkg\"\nversion = \"1.2.3\"\n",
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

        let manifest = open(&decl, &ctx).expect("open() should dispatch PyprojectToml to PyprojectToml::open");
        assert_eq!(manifest.ecosystem(), Ecosystem::Pypi);
        assert_eq!(manifest.role(), ManifestRole::Canonical);
        assert_eq!(manifest.package_name().unwrap(), "demo-pkg");
        assert_eq!(manifest.current_version().unwrap().render(), "1.2.3");
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn read_identity_extracts_cargo_name_and_literal_version() {
        let identity = read_identity(
            ManifestFormat::CargoToml,
            "[package]\nname = \"my-crate\"\nversion = \"1.2.3\"\n",
            Path::new("Cargo.toml"),
        )
        .unwrap();
        assert_eq!(identity.name.as_deref(), Some("my-crate"));
        assert_eq!(identity.version, Some(VersionSource::Literal("1.2.3".to_string())));
    }

    #[test]
    fn read_identity_detects_cargo_inherited_version() {
        let identity = read_identity(
            ManifestFormat::CargoToml,
            "[package]\nname = \"my-crate\"\nversion.workspace = true\n",
            Path::new("Cargo.toml"),
        )
        .unwrap();
        assert_eq!(identity.name.as_deref(), Some("my-crate"));
        assert_eq!(identity.version, Some(VersionSource::InheritedFromWorkspace));
    }

    #[test]
    fn read_identity_extracts_npm_name_and_version() {
        let identity = read_identity(
            ManifestFormat::PackageJson,
            r#"{"name":"my-pkg","version":"1.0.0"}"#,
            Path::new("package.json"),
        )
        .unwrap();
        assert_eq!(identity.name.as_deref(), Some("my-pkg"));
        assert_eq!(identity.version, Some(VersionSource::Literal("1.0.0".to_string())));
    }

    #[test]
    fn read_identity_extracts_pypi_name_via_flit_fallback() {
        let identity = read_identity(
            ManifestFormat::PyprojectToml,
            "[tool.flit.metadata]\nmodule = \"my_flit_lib\"\n",
            Path::new("pyproject.toml"),
        )
        .unwrap();
        assert_eq!(identity.name.as_deref(), Some("my_flit_lib"));
        assert_eq!(identity.version, None);
    }

    #[test]
    fn read_identity_extracts_pypi_version_via_poetry_fallback() {
        let identity = read_identity(
            ManifestFormat::PyprojectToml,
            "[tool.poetry]\nname = \"my-lib\"\nversion = \"2.0.0\"\n",
            Path::new("pyproject.toml"),
        )
        .unwrap();
        assert_eq!(identity.version, Some(VersionSource::Literal("2.0.0".to_string())));
    }

    #[test]
    fn read_identity_rejects_non_canonical_format() {
        let err = read_identity(ManifestFormat::GoMod, "module foo\n", Path::new("go.mod")).unwrap_err();
        assert!(matches!(err, ManifestError::ReadOnlyFormat { .. }));
    }

    #[test]
    fn read_identity_returns_parse_error_on_malformed_source() {
        let err = read_identity(ManifestFormat::CargoToml, "not valid = [ toml", Path::new("Cargo.toml")).unwrap_err();
        assert!(matches!(err, ManifestError::Parse { .. }));
    }

    #[test]
    fn read_napi_targets_absent_when_no_napi_key() {
        let val: serde_json::Value = serde_json::json!({"name": "pkg"});
        let result = read_napi_targets(Path::new("package.json"), &val).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn read_napi_targets_present_array() {
        let val: serde_json::Value = serde_json::json!({"napi": {"targets": ["x86_64-apple-darwin"]}});
        let result = read_napi_targets(Path::new("package.json"), &val).unwrap();
        assert_eq!(result, Some(vec!["x86_64-apple-darwin".to_string()]));
    }

    #[test]
    fn read_napi_targets_errors_on_non_array() {
        let val: serde_json::Value = serde_json::json!({"napi": {"targets": "not-an-array"}});
        let result = read_napi_targets(Path::new("package.json"), &val);
        assert!(result.is_err());
    }

    #[test]
    fn read_napi_targets_errors_on_non_string_entries() {
        let val: serde_json::Value = serde_json::json!({"napi": {"targets": ["x64", 42]}});
        let result = read_napi_targets(Path::new("package.json"), &val);
        assert!(result.is_err());
    }
}
