use std::collections::HashSet;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    workspace_relative, Ecosystem, ModelError, PackageId, PublishTarget, ReleaseTrigger, TagTemplate, VersionGrammar,
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
        self.manifests.iter().filter(|m| m.role == ManifestRole::Canonical)
    }

    pub fn platform_manifests(&self) -> impl Iterator<Item = &ManifestDecl> {
        self.manifests
            .iter()
            .filter(|m| matches!(m.role, ManifestRole::Platform { .. }))
    }

    pub fn lockfiles(&self) -> impl Iterator<Item = &ManifestDecl> {
        self.manifests.iter().filter(|m| m.role == ManifestRole::Lockfile)
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
        self.publish_to.iter().any(|t| !matches!(t, PublishTarget::None))
    }

    pub fn is_dual_published(&self) -> bool {
        let canonical_ecosystems: HashSet<_> = self.canonical_manifests().map(|m| m.format.ecosystem()).collect();
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
    pub fn new(path: impl AsRef<Path>, role: ManifestRole, format: ManifestFormat) -> Result<Self, ModelError> {
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
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
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

    pub fn from_path(p: &std::path::Path) -> Result<Self, ModelError> {
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| ModelError::UnknownManifestFormat { path: p.to_path_buf() })?;
        match name {
            "Cargo.toml" => Ok(ManifestFormat::CargoToml),
            "package.json" => Ok(ManifestFormat::PackageJson),
            "pyproject.toml" => Ok(ManifestFormat::PyprojectToml),
            "setup.cfg" => Ok(ManifestFormat::SetupCfg),
            "go.mod" => Ok(ManifestFormat::GoMod),
            "pom.xml" => Ok(ManifestFormat::PomXml),
            "deno.json" | "deno.jsonc" => Ok(ManifestFormat::DenoJson),
            _ => Err(ModelError::UnknownManifestFormat { path: p.to_path_buf() }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_manifest_role_and_format() {
        let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml);
        assert!(decl.is_ok());

        let invalid = ManifestDecl::new("Cargo.toml", ManifestRole::Lockfile, ManifestFormat::CargoToml);
        assert!(invalid.is_err());
    }

    #[test]
    fn manifest_decl_new_rejects_lockfile_format_with_non_lockfile_role() {
        // The opposite direction of validates_manifest_role_and_format's
        // check: a genuinely lockfile-shaped format (Cargo.lock) declared
        // under a non-Lockfile role must also be rejected.
        let err = ManifestDecl::new("Cargo.lock", ManifestRole::Canonical, ManifestFormat::CargoLock).unwrap_err();
        assert!(matches!(err, ModelError::InvalidRoleForFormat { .. }));
    }

    #[test]
    fn manifest_decl_new_rejects_platform_role_with_cargo_toml_format() {
        // Platform-role manifests are napi/maturin platform-stub packages
        // (package.json/pyproject.toml); Cargo.toml is never a Platform
        // manifest, since Cargo has no equivalent platform-stub-package
        // convention.
        let err = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Platform {
                platform: "darwin".to_string(),
                arch: "arm64".to_string(),
                abi: None,
            },
            ManifestFormat::CargoToml,
        )
        .unwrap_err();
        assert!(matches!(err, ModelError::InvalidRoleForFormat { .. }));
    }

    fn test_package(manifests: Vec<ManifestDecl>, publish_to: Vec<PublishTarget>) -> Package {
        Package {
            id: PackageId::Bare("pkg".to_string()),
            manifests,
            changelog: None,
            release_trigger: ReleaseTrigger::Changeset,
            publish_to,
            tag_template: None,
        }
    }

    #[test]
    fn version_grammar_returns_the_single_canonical_ecosystems_grammar() {
        let cargo = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
        let pkg = test_package(vec![cargo], vec![]);
        assert_eq!(pkg.version_grammar().unwrap(), VersionGrammar::SemVer);
    }

    #[test]
    fn version_grammar_errors_with_no_canonical_manifest() {
        let pkg = test_package(vec![], vec![]);
        let err = pkg.version_grammar().unwrap_err();
        assert!(matches!(err, ModelError::NoCanonicalManifest { .. }), "got {err:?}");
    }

    #[test]
    fn version_grammar_errors_on_mixed_grammars_across_canonical_manifests() {
        let cargo = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
        let pypi = ManifestDecl::new("pyproject.toml", ManifestRole::Canonical, ManifestFormat::PyprojectToml).unwrap();
        let pkg = test_package(vec![cargo, pypi], vec![]);

        let err = pkg.version_grammar().unwrap_err();
        assert!(matches!(err, ModelError::MixedVersionGrammars { .. }), "got {err:?}");
    }

    #[test]
    fn platform_manifests_filters_to_platform_role_only() {
        let canonical =
            ManifestDecl::new("package.json", ManifestRole::Canonical, ManifestFormat::PackageJson).unwrap();
        let platform = ManifestDecl::new(
            "npm/darwin-arm64/package.json",
            ManifestRole::Platform {
                platform: "darwin".to_string(),
                arch: "arm64".to_string(),
                abi: None,
            },
            ManifestFormat::PackageJson,
        )
        .unwrap();
        let pkg = test_package(vec![canonical, platform.clone()], vec![]);

        let found: Vec<_> = pkg.platform_manifests().collect();
        assert_eq!(found, vec![&platform]);
    }

    #[test]
    fn lockfiles_filters_to_lockfile_role_only() {
        let canonical = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
        let lockfile = ManifestDecl::new("Cargo.lock", ManifestRole::Lockfile, ManifestFormat::CargoLock).unwrap();
        let pkg = test_package(vec![canonical, lockfile.clone()], vec![]);

        let found: Vec<_> = pkg.lockfiles().collect();
        assert_eq!(found, vec![&lockfile]);
    }

    #[test]
    fn is_release_point_true_when_any_publish_target_is_not_none() {
        let pkg = test_package(vec![], vec![PublishTarget::None, PublishTarget::CratesIo]);
        assert!(pkg.is_release_point());
    }

    #[test]
    fn is_release_point_false_when_all_publish_targets_are_none() {
        let pkg = test_package(vec![], vec![PublishTarget::None]);
        assert!(!pkg.is_release_point());

        let pkg_empty = test_package(vec![], vec![]);
        assert!(!pkg_empty.is_release_point());
    }

    #[test]
    fn is_dual_published_true_with_two_distinct_canonical_ecosystems() {
        let cargo = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
        let npm = ManifestDecl::new("package.json", ManifestRole::Canonical, ManifestFormat::PackageJson).unwrap();
        let pkg = test_package(vec![cargo, npm], vec![]);

        assert!(pkg.is_dual_published());
    }

    #[test]
    fn is_dual_published_false_with_a_single_canonical_ecosystem() {
        let cargo = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
        let pkg = test_package(vec![cargo], vec![]);

        assert!(!pkg.is_dual_published());
    }

    #[test]
    fn manifest_format_file_name_and_is_writable_cover_every_variant() {
        let cases: &[(ManifestFormat, &str, bool)] = &[
            (ManifestFormat::CargoToml, "Cargo.toml", true),
            (ManifestFormat::PackageJson, "package.json", true),
            (ManifestFormat::PyprojectToml, "pyproject.toml", true),
            (ManifestFormat::SetupCfg, "setup.cfg", false),
            (ManifestFormat::GoMod, "go.mod", true),
            (ManifestFormat::PomXml, "pom.xml", true),
            (ManifestFormat::GradleVersionCatalog, "libs.versions.toml", true),
            (ManifestFormat::SettingsGradle, "settings.gradle", false),
            (ManifestFormat::VersionSbt, "build.sbt", false),
            (ManifestFormat::DenoJson, "deno.json", true),
            (ManifestFormat::CargoLock, "Cargo.lock", false),
            (ManifestFormat::PackageLockJson, "package-lock.json", false),
            (ManifestFormat::PnpmLockYaml, "pnpm-lock.yaml", false),
            (ManifestFormat::YarnLock, "yarn.lock", false),
        ];
        for &(format, expected_name, expected_writable) in cases {
            assert_eq!(format.file_name(), expected_name, "file_name mismatch for {format:?}");
            assert_eq!(
                format.is_writable(),
                expected_writable,
                "is_writable mismatch for {format:?}"
            );
        }
    }

    #[test]
    fn manifest_format_from_path_reads_recognized_names_and_rejects_unknown() {
        assert_eq!(
            ManifestFormat::from_path(std::path::Path::new("Cargo.toml")).unwrap(),
            ManifestFormat::CargoToml
        );
        assert_eq!(
            ManifestFormat::from_path(std::path::Path::new("deno.jsonc")).unwrap(),
            ManifestFormat::DenoJson
        );

        let err = ManifestFormat::from_path(std::path::Path::new("Makefile")).unwrap_err();
        assert!(matches!(err, ModelError::UnknownManifestFormat { .. }));
    }
}
