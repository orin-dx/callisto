//! Per-ecosystem manifest read/write for callisto.

use std::path::Path;
use std::sync::Arc;

use callisto_model::{
    DepSpec, Ecosystem, ManifestDecl, ManifestError, ManifestFormat, ManifestRole, Version,
    WorkspaceKind,
};

pub mod atomic;
pub use atomic::ChangesetStorage;

pub mod cargo;
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
    fn write_version(&mut self, v: &Version) -> Result<(), ManifestError>;
    fn iter_dependencies(&self) -> Box<dyn Iterator<Item = callisto_model::DependencyEntry> + '_>;
    fn update_dependency_spec(
        &mut self,
        name: &str,
        kind: callisto_model::DepKind,
        new: DepSpec,
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
            Ecosystem::Npm => vec![callisto_model::PublishTarget::Npm { registry: None }],
            _ => vec![callisto_model::PublishTarget::None],
        }
    }
    fn update_optional_dependencies(
        &mut self,
        updates: &[(String, Version)],
    ) -> Result<(), ManifestError>;
}

/// Trait for Concrete Syntax Tree (CST) manifest editors that preserve formatting, comments, and key order.
pub trait ManifestCstEditor {
    fn update_version_cst(&mut self, new_version: &Version) -> Result<(), ManifestError>;
    fn update_dependency_cst(
        &mut self,
        name: &str,
        kind: callisto_model::DepKind,
        new_spec: DepSpec,
    ) -> Result<(), ManifestError>;
}

impl<T: Manifest + ?Sized> ManifestCstEditor for T {
    fn update_version_cst(&mut self, new_version: &Version) -> Result<(), ManifestError> {
        self.write_version(new_version)
    }

    fn update_dependency_cst(
        &mut self,
        name: &str,
        kind: callisto_model::DepKind,
        new_spec: DepSpec,
    ) -> Result<(), ManifestError> {
        self.update_dependency_spec(name, kind, new_spec)
    }
}

/// Context passed to open() to supply workspace-wide inheritance facts.
pub struct OpenContext<'a> {
    pub workspace_root: &'a Path,
    pub cargo_workspace: Option<Arc<WorkspaceInheritance>>,
    pub npm_workspace_kind: Option<WorkspaceKind>,
}

/// Opens a manifest file matching `decl.format`.
pub fn open(
    decl: &ManifestDecl,
    ctx: &OpenContext<'_>,
) -> Result<Box<dyn Manifest>, ManifestError> {
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
        _ => None,
    }
}
