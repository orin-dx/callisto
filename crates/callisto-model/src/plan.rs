use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CommitSha, Diagnostic, PackageId, RegistryKey, TagName, Version};

/// Complete publish plan output for plan-publish command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublishPlan {
    pub schema_version: u32,
    pub rust_crates: Vec<CratePublish>,
    pub npm_platform_packages: Vec<NpmPublish>,
    pub npm_main_packages: Vec<NpmMainPublish>,
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NpmPublish {
    pub name: String,
    pub version: Version,
    pub publish_to: RegistryKey,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NpmMainPublish {
    pub name: String,
    pub version: Version,
    pub publish_to: RegistryKey,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,

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
