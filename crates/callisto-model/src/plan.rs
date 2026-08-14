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
