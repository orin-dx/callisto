//! Per-ecosystem manifest read/write for callisto.

use std::path::Path;
use std::sync::Arc;

use callisto_model::{
    ApplyPermit, DepSpec, Ecosystem, ManifestDecl, ManifestError, ManifestFormat, ManifestRole,
    Version, WorkspaceKind,
};

pub mod atomic;
pub use atomic::ChangesetStorage;

pub mod cargo;
mod common;
pub mod npm;
pub mod python;

pub use cargo::{CargoToml, InheritedDep, WorkspaceCargoResolver, WorkspaceInheritance};
pub use npm::{detect_npm_workspace_kind, PackageJson};
pub use python::PyprojectToml;

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
                restricted: false,
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

/// Trait for Concrete Syntax Tree (CST) manifest editors that preserve formatting, comments, and key order.
pub trait ManifestCstEditor {
    fn update_version_cst(
        &mut self,
        new_version: &Version,
        permit: &ApplyPermit,
    ) -> Result<(), ManifestError>;
    fn update_dependency_cst(
        &mut self,
        name: &str,
        kind: callisto_model::DepKind,
        new_spec: DepSpec,
        permit: &ApplyPermit,
    ) -> Result<(), ManifestError>;
}

impl<T: Manifest + ?Sized> ManifestCstEditor for T {
    fn update_version_cst(
        &mut self,
        new_version: &Version,
        permit: &ApplyPermit,
    ) -> Result<(), ManifestError> {
        self.write_version(new_version, permit)
    }

    fn update_dependency_cst(
        &mut self,
        name: &str,
        kind: callisto_model::DepKind,
        new_spec: DepSpec,
        permit: &ApplyPermit,
    ) -> Result<(), ManifestError> {
        self.update_dependency_spec(name, kind, new_spec, permit)
    }
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

/// Opens a manifest file matching `decl.format`.
pub fn open(
    decl: &ManifestDecl,
    ctx: &OpenContext<'_>,
) -> Result<Box<dyn Manifest>, ManifestError> {
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

        let manifest =
            open(&decl, &ctx).expect("open() should dispatch PyprojectToml to PyprojectToml::open");
        assert_eq!(manifest.ecosystem(), Ecosystem::Pypi);
        assert_eq!(manifest.role(), ManifestRole::Canonical);
        assert_eq!(manifest.package_name().unwrap(), "demo-pkg");
        assert_eq!(manifest.current_version().unwrap().render(), "1.2.3");
    }
}
