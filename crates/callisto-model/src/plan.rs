use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CommitSha, Diagnostic, PackageId, RegistryKey, TagName, Version};

/// npm package access level, controlling the `--access` flag passed to `npm publish`.
///
/// When `None` is set on a publish entry, no `--access` flag is passed and
/// npm uses its default: `restricted` for scoped packages (`@scope/name`),
/// `public` for unscoped packages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NpmAccess {
    /// Publish as publicly accessible. Required for scoped packages that
    /// should be available without authentication.
    Public,
    /// Publish as restricted (private). Only accessible to authorized users
    /// and teams. This is npm's default for scoped packages.
    Restricted,
}

/// Complete publish plan output for plan-publish command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublishPlan {
    pub schema_version: u32,
    pub rust_crates: Vec<CratePublish>,
    pub npm_platform_packages: Vec<NpmPublish>,
    pub npm_main_packages: Vec<NpmMainPublish>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pypi_packages: Vec<PypiPublish>,

    pub releases: Vec<ReleaseEntry>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl PublishPlan {
    /// `true` when every publishable list and `releases` is empty — nothing
    /// in this plan actually needs to run `callisto publish` or `callisto
    /// tag`. Callers driving a release pipeline (CI orchestration in
    /// particular) should check this before running either, rather than
    /// unconditionally reporting `published=true` for a run that shipped
    /// nothing.
    pub fn is_empty(&self) -> bool {
        self.rust_crates.is_empty()
            && self.npm_platform_packages.is_empty()
            && self.npm_main_packages.is_empty()
            && self.pypi_packages.is_empty()
            && self.releases.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CratePublish {
    pub name: String,
    pub version: Version,
    pub publish_to: RegistryKey,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,

    /// Directory of this package relative to the workspace root (e.g.
    /// `"crates/my-crate"`). Used to locate the on-disk `Cargo.toml` for a
    /// pre-publish version sanity check. Absent in older plan files — treated
    /// as unknown package location, skipping the version check in that case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NpmPublish {
    pub name: String,
    pub version: Version,
    pub publish_to: RegistryKey,
    /// Directory of this package relative to the workspace root (e.g.
    /// `"packages/my-pkg"`). Required by package managers that run from the
    /// package directory (bun) rather than the workspace root.
    pub package_dir: PathBuf,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,

    /// Explicit npm access level. When `None`, no `--access` flag is passed
    /// and npm uses its ecosystem default (restricted for scoped packages,
    /// public for unscoped). Set to `Some(NpmAccess::Public)` only when the
    /// package must be published as publicly accessible (e.g. a scoped package
    /// that should be available without authentication).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<NpmAccess>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NpmMainPublish {
    pub name: String,
    pub version: Version,
    pub publish_to: RegistryKey,
    /// Directory of this package relative to the workspace root. See
    /// [`NpmPublish::package_dir`] for semantics.
    pub package_dir: PathBuf,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,

    /// Explicit npm access level. See [`NpmPublish::access`] for semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<NpmAccess>,

    pub depends_on_platforms: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseEntry {
    pub package: PackageId,
    pub tag_name: TagName,
    pub sha: CommitSha,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changelog_section: Option<String>,

    pub is_prerelease: bool,
}

/// A single package scheduled for publication to PyPI (or a compatible index).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PypiPublish {
    /// Normalized distribution name (e.g. `"callisto-py"`).
    pub name: String,
    pub version: Version,
    pub publish_to: RegistryKey,
    /// Directory of this package relative to the workspace root (e.g.
    /// `"packages/callisto-py"`). `twine upload` and `python -m build` run
    /// from this directory so that `dist/` resolves correctly in a monorepo.
    pub package_dir: PathBuf,

    /// Custom index URL passed to `twine upload --repository-url`. `None`
    /// targets the default public PyPI index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_plan() -> PublishPlan {
        PublishPlan {
            schema_version: 1,
            rust_crates: vec![],
            npm_platform_packages: vec![],
            npm_main_packages: vec![],
            pypi_packages: vec![],
            releases: vec![],
            diagnostics: vec![],
        }
    }

    #[test]
    fn is_empty_true_for_fully_empty_plan() {
        assert!(empty_plan().is_empty());
    }

    #[test]
    fn is_empty_false_when_rust_crates_nonempty() {
        let mut plan = empty_plan();
        plan.rust_crates.push(CratePublish {
            name: "pkg".to_string(),
            version: crate::Version::parse("1.0.0", crate::VersionGrammar::SemVer).unwrap(),
            publish_to: RegistryKey(RegistryKey::CRATES_IO.to_string()),
            registry: None,
            package_dir: None,
        });
        assert!(!plan.is_empty());
    }

    /// Guards against the exact bug the field-list-completeness in `is_empty`
    /// itself exists to prevent -- a check that forgets one field would
    /// wrongly report `true` for a plan whose only content is a `pypi_packages`
    /// entry (before this method existed, the CLI's own inline duplicate of
    /// this check listed every field explicitly, and the risk is real given
    /// `pypi_packages` is the newest of the five and easiest to omit).
    #[test]
    fn is_empty_false_when_only_pypi_packages_nonempty() {
        let mut plan = empty_plan();
        plan.pypi_packages.push(PypiPublish {
            name: "pkg".to_string(),
            version: crate::Version::parse("1.0.0", crate::VersionGrammar::SemVer).unwrap(),
            publish_to: RegistryKey(RegistryKey::PYPI.to_string()),
            package_dir: PathBuf::from("pkg"),
            index: None,
        });
        assert!(!plan.is_empty());
    }

    #[test]
    fn is_empty_false_for_release_only_plan() {
        let mut plan = empty_plan();
        plan.releases.push(ReleaseEntry {
            package: crate::PackageId::Bare("pkg".to_string()),
            tag_name: crate::TagName("pkg@1.0.0".to_string()),
            sha: crate::CommitSha::parse("a".repeat(40).as_str()).unwrap(),
            changelog_section: None,
            is_prerelease: false,
        });
        assert!(!plan.is_empty());
    }
}
