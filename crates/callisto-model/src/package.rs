use std::collections::HashSet;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    workspace_relative, Ecosystem, ModelError, PackageId, PublishTarget, ReleaseTrigger,
    TagTemplate, VersionGrammar,
};

/// A package in the workspace graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub id: PackageId,
    pub manifests: Vec<ManifestDecl>,
    pub changelog: Option<PathBuf>,
    pub release_trigger: ReleaseTrigger,
    pub publish_to: Vec<PublishTarget>,
    pub tag_template: Option<TagTemplate>,
}

impl Package {
    pub fn canonical_manifests(&self) -> impl Iterator<Item = &ManifestDecl> {
        self.manifests
            .iter()
            .filter(|m| m.role == ManifestRole::Canonical)
    }

    pub fn platform_manifests(&self) -> impl Iterator<Item = &ManifestDecl> {
        self.manifests
            .iter()
            .filter(|m| matches!(m.role, ManifestRole::Platform { .. }))
    }

    pub fn lockfiles(&self) -> impl Iterator<Item = &ManifestDecl> {
        self.manifests
            .iter()
            .filter(|m| m.role == ManifestRole::Lockfile)
    }

    pub fn version_grammar(&self) -> Result<VersionGrammar, ModelError> {
        let grammars: Vec<_> = self
            .canonical_manifests()
            .map(|m| m.format.ecosystem().version_grammar())
            .collect();

        if grammars.is_empty() {
            return Err(ModelError::no_canonical_manifest(&self.id));
        }

        let first = grammars[0];
        if grammars.iter().any(|&g| g != first) {
            let unique: HashSet<_> = grammars.into_iter().collect();
            return Err(ModelError::mixed_version_grammars(
                &self.id,
                unique.into_iter().collect(),
            ));
        }

        Ok(first)
    }

    pub fn is_release_point(&self) -> bool {
        self.publish_to
            .iter()
            .any(|t| !matches!(t, PublishTarget::None))
    }

    pub fn is_dual_published(&self) -> bool {
        let canonical_ecosystems: HashSet<_> = self
            .canonical_manifests()
            .map(|m| m.format.ecosystem())
            .collect();
        canonical_ecosystems.len() >= 2
    }
}

/// Declaration of a manifest file.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ManifestDecl {
    pub path: PathBuf,
    pub role: ManifestRole,
    pub format: ManifestFormat,
}

impl ManifestDecl {
    pub fn new(
        path: impl AsRef<Path>,
        role: ManifestRole,
        format: ManifestFormat,
    ) -> Result<Self, ModelError> {
        let rel_path = workspace_relative(path)?;

        if format.is_lockfile() && role != ManifestRole::Lockfile {
            return Err(ModelError::invalid_role_for_format(&role, &format));
        }

        if role == ManifestRole::Lockfile && !format.is_lockfile() {
            return Err(ModelError::invalid_role_for_format(&role, &format));
        }

        if matches!(role, ManifestRole::Platform { .. }) && format == ManifestFormat::CargoToml {
            return Err(ModelError::invalid_role_for_format(&role, &format));
        }

        Ok(ManifestDecl {
            path: rel_path,
            role,
            format,
        })
    }

    pub fn ecosystem(&self) -> Ecosystem {
        self.format.ecosystem()
    }
}

/// Role of a manifest file in a package.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ManifestRole {
    Canonical,
    Platform {
        platform: String,
        arch: String,
        abi: Option<String>,
    },
    Lockfile,
}

/// Format of a manifest file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ManifestFormat {
    CargoToml,
    PackageJson,
    PyprojectToml,
    SetupCfg,
    GoMod,
    PomXml,
    GradleVersionCatalog,
    SettingsGradle,
    VersionSbt,
    DenoJson,
    CargoLock,
    PackageLockJson,
    PnpmLockYaml,
    YarnLock,
}

impl ManifestFormat {
    pub fn ecosystem(&self) -> Ecosystem {
        match self {
            ManifestFormat::CargoToml | ManifestFormat::CargoLock => Ecosystem::Cargo,
            ManifestFormat::PackageJson
            | ManifestFormat::PackageLockJson
            | ManifestFormat::PnpmLockYaml
            | ManifestFormat::YarnLock => Ecosystem::Npm,
            ManifestFormat::PyprojectToml | ManifestFormat::SetupCfg => Ecosystem::Pypi,
            ManifestFormat::GoMod => Ecosystem::Go,
            ManifestFormat::PomXml
            | ManifestFormat::GradleVersionCatalog
            | ManifestFormat::SettingsGradle
            | ManifestFormat::VersionSbt => Ecosystem::Maven,
            ManifestFormat::DenoJson => Ecosystem::Deno,
        }
    }

    pub fn is_lockfile(&self) -> bool {
        matches!(
            self,
            ManifestFormat::CargoLock
                | ManifestFormat::PackageLockJson
                | ManifestFormat::PnpmLockYaml
                | ManifestFormat::YarnLock
        )
    }

    pub fn is_writable(&self) -> bool {
        !matches!(
            self,
            ManifestFormat::SetupCfg
                | ManifestFormat::CargoLock
                | ManifestFormat::PackageLockJson
                | ManifestFormat::PnpmLockYaml
                | ManifestFormat::YarnLock
                | ManifestFormat::SettingsGradle
                | ManifestFormat::VersionSbt
        )
    }

    pub fn file_name(&self) -> &'static str {
        match self {
            ManifestFormat::CargoToml => "Cargo.toml",
            ManifestFormat::PackageJson => "package.json",
            ManifestFormat::PyprojectToml => "pyproject.toml",
            ManifestFormat::SetupCfg => "setup.cfg",
            ManifestFormat::GoMod => "go.mod",
            ManifestFormat::PomXml => "pom.xml",
            ManifestFormat::GradleVersionCatalog => "libs.versions.toml",
            ManifestFormat::SettingsGradle => "settings.gradle",
            ManifestFormat::VersionSbt => "build.sbt",
            ManifestFormat::DenoJson => "deno.json",
            ManifestFormat::CargoLock => "Cargo.lock",
            ManifestFormat::PackageLockJson => "package-lock.json",
            ManifestFormat::PnpmLockYaml => "pnpm-lock.yaml",
            ManifestFormat::YarnLock => "yarn.lock",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_manifest_role_and_format() {
        let decl = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        );
        assert!(decl.is_ok());

        let invalid = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Lockfile,
            ManifestFormat::CargoToml,
        );
        assert!(invalid.is_err());
    }
}
