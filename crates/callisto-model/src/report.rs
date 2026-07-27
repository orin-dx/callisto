use std::path::PathBuf;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{
    CommitSha, ConfigKey, DepKind, Diagnostic, GroupName, PackageId, PublishPlan, Severity,
    TagName, Version,
};

pub const SCHEMA_VERSION: u32 = 1;

/// Trait for all structured JSON report payloads.
pub trait Report: Serialize + DeserializeOwned + Send + Sync + 'static {
    const COMMAND: &'static str;
    fn schema_version(&self) -> u32;
    fn diagnostics(&self) -> &[Diagnostic];
}

impl Report for PublishPlan {
    const COMMAND: &'static str = "plan-publish";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

/// Version report output from `callisto version --format json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VersionReport {
    pub schema_version: u32,
    pub bumps: Vec<BumpRecord>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lockfile_refresh_results: Option<Vec<LockfileRefreshResult>>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl Report for VersionReport {
    const COMMAND: &'static str = "version";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BumpRecord {
    pub package: PackageId,
    pub from: Version,
    pub to: Version,
    pub severity: Severity,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governed_by: Option<ConfigKey>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<BumpReason>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[non_exhaustive]
pub enum BumpReason {
    Changeset {
        changesets: Vec<String>,
    },
    Inference {
        commits: usize,
        remapped: bool,
    },
    FixedGroupUnion {
        group: GroupName,
    },
    LinkedGroupUnion {
        group: GroupName,
    },
    Cascade {
        via: PackageId,
        dep_kind: DepKind,
        spec: String,
        dependency_to: Version,
    },
    PeerEscalation {
        via: PackageId,
        spec: String,
    },
    PreRelease {
        tag: String,
    },
    NewGroupMember {
        group: GroupName,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LockfileRefreshResult {
    pub filename: PathBuf,
    pub refresh_command: String,
    pub success: bool,
    pub exit_code: Option<i32>,
}

/// Status report output from `callisto status --format json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatusReport {
    pub schema_version: u32,
    pub packages: Vec<StatusPackageRecord>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl Report for StatusReport {
    const COMMAND: &'static str = "status";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatusPackageRecord {
    pub package: PackageId,
    pub current_version: Version,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tag: Option<TagName>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_severity: Option<Severity>,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub changed_since_last_tag: bool,

    pub pending_changesets: Vec<String>,
}

/// Snapshot report output from `callisto snapshot --format json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotReport {
    pub schema_version: u32,
    pub snapshot_tag: String,
    pub bumps: Vec<BumpRecord>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl Report for SnapshotReport {
    const COMMAND: &'static str = "snapshot";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

/// Compose PR body report output from `callisto compose-pr-body --format json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ComposePrBodyReport {
    pub schema_version: u32,
    pub pr_body: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl Report for ComposePrBodyReport {
    const COMMAND: &'static str = "compose-pr-body";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

/// Validate report output from `callisto validate --format json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidateReport {
    pub schema_version: u32,
    pub valid: bool,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl Report for ValidateReport {
    const COMMAND: &'static str = "validate";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

/// Tag report output from `callisto tag --format json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TagReport {
    pub schema_version: u32,
    pub created_tags: Vec<CreatedTag>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl Report for TagReport {
    const COMMAND: &'static str = "tag";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreatedTag {
    pub package: PackageId,
    pub tag_name: TagName,
    pub sha: CommitSha,
}

/// Init report output from `callisto init --format json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitReport {
    pub schema_version: u32,
    pub initialized: bool,
    pub config_path: PathBuf,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl Report for InitReport {
    const COMMAND: &'static str = "init";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}
