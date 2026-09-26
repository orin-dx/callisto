use std::path::PathBuf;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{ConfigKey, DepKind, Diagnostic, GroupName, PackageId, ReleaseTrigger, Severity, TagName, Version};

pub const SCHEMA_VERSION: u32 = 1;

/// Trait for all structured JSON report payloads.
pub trait Report: Serialize + DeserializeOwned + Send + Sync + 'static {
    const COMMAND: &'static str;
    fn schema_version(&self) -> u32;
    fn diagnostics(&self) -> &[Diagnostic];
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
    /// Mandatory — the field the Action's mode dispatch reads.
    /// Always serialized, never omitted, even when `false`.
    pub has_changesets: bool,
    /// Count of packages with a planned bump (`StatusPackageRecord.pending_severity.is_some()`),
    /// i.e. post-cascade/fixed/linked-group -- unlike `has_changesets`, which only reflects
    /// changeset files directly naming a package. `status --check`'s
    /// exit code no longer signals pending state (0/1 gate on errors only), so this field is
    /// how a script detects it. Always serialized, even when `0`.
    pub pending: u32,
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

    pub last_released_version: Option<Version>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_severity: Option<Severity>,

    /// Mandatory, always serialized — empty-changeset validation and
    /// `changed_since_last_tag` consumers depend on this being present even
    /// when `false`.
    pub changed_since_last_tag: bool,

    pub release_trigger: ReleaseTrigger,

    pub pending_changesets: Vec<String>,
}

#[cfg(test)]
mod status_report_tests {
    use super::*;
    use crate::{PackageId, ReleaseTrigger, Version, VersionGrammar};

    fn pkg() -> PackageId {
        PackageId::parse("test-pkg").unwrap()
    }

    fn ver() -> Version {
        Version::parse("1.0.0", VersionGrammar::SemVer).unwrap()
    }

    fn record() -> StatusPackageRecord {
        StatusPackageRecord {
            package: pkg(),
            current_version: ver(),
            last_tag: None,
            last_released_version: None,
            pending_severity: None,
            changed_since_last_tag: false,
            release_trigger: ReleaseTrigger::Changeset,
            pending_changesets: vec![],
        }
    }

    /// `StatusReport.hasChangesets` is mandatory — the Action's mode dispatch
    /// reads it — so it must always be present in the serialized JSON, never
    /// omitted regardless of its value.
    #[test]
    fn status_report_json_always_contains_has_changesets() {
        let report = StatusReport {
            schema_version: SCHEMA_VERSION,
            has_changesets: false,
            pending: 0,
            packages: vec![],
            diagnostics: vec![],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(
            json.contains("\"hasChangesets\":false"),
            "StatusReport JSON must always contain hasChangesets, even when false; got: {json}"
        );
    }

    /// `pending` (count of packages with a planned
    /// bump) must always serialize, including when `0` -- it's the field a
    /// script now reads to detect pending changesets, since `status --check`'s
    /// exit code no longer signals it.
    #[test]
    fn status_report_json_always_contains_pending() {
        let report = StatusReport {
            schema_version: SCHEMA_VERSION,
            has_changesets: false,
            pending: 0,
            packages: vec![],
            diagnostics: vec![],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(
            json.contains("\"pending\":0"),
            "StatusReport JSON must always contain pending, even when 0; got: {json}"
        );
    }

    /// `StatusEntry.changedSinceLastTag` is
    /// mandatory and computed from v0.1 — it must always be serialized,
    /// including when `false`, not omitted via skip_serializing_if.
    #[test]
    fn status_package_record_json_always_contains_changed_since_last_tag() {
        let rec = record();
        let json = serde_json::to_string(&rec).unwrap();
        assert!(
            json.contains("\"changedSinceLastTag\":false"),
            "StatusPackageRecord JSON must always contain changedSinceLastTag, even when \
             false; got: {json}"
        );
    }

    /// `StatusEntry` carries `lastReleasedVersion`
    /// and `releaseTrigger` fields alongside the other five.
    #[test]
    fn status_package_record_carries_last_released_version_and_release_trigger() {
        let rec = StatusPackageRecord {
            last_released_version: Some(ver()),
            release_trigger: ReleaseTrigger::Auto,
            ..record()
        };
        let json = serde_json::to_string(&rec).unwrap();
        assert!(
            json.contains("\"lastReleasedVersion\":\"1.0.0\""),
            "StatusPackageRecord JSON must carry lastReleasedVersion; got: {json}"
        );
        assert!(
            json.contains("\"releaseTrigger\":\"auto\""),
            "StatusPackageRecord JSON must carry releaseTrigger; got: {json}"
        );
    }
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
    /// The composed PR body. Wire key is `body`
    /// (not `prBody`).
    ///
    /// NOTE: the original spec also documented a `metadata:
    /// PrBodyMetadata` field. It is intentionally not present on this
    /// struct yet -- see the `compose_pr_body_report_json_uses_body_key_not_pr_body`
    /// test doc comment for why.
    pub body: String,

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

#[cfg(test)]
mod compose_pr_body_report_tests {
    use super::*;

    /// `ComposePrBodyReport`'s composed-body field is
    /// documented as `body`, serialized camelCase as `"body"` -- not `"prBody"`.
    ///
    /// NOTE: intentionally doesn't assert a `metadata` key. The original spec also
    /// documented `metadata: PrBodyMetadata` (labels/managedLabels/overflow),
    /// but the data to populate `managedLabels` (round-tripping the
    /// previous run's labels) and `overflow` (notes-branch overflow
    /// detection) isn't computed anywhere in
    /// `callisto-graph::commands::pr_body` today. Adding it here would
    /// mean fabricating placeholder data -- out of scope, left for
    /// separate work.
    #[test]
    fn compose_pr_body_report_json_uses_body_key_not_pr_body() {
        let report = ComposePrBodyReport {
            schema_version: SCHEMA_VERSION,
            body: "## Release Preview".to_string(),
            diagnostics: vec![],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(
            json.contains("\"body\":\"## Release Preview\""),
            "ComposePrBodyReport JSON must contain the \"body\" key; got: {json}"
        );
        assert!(
            !json.contains("\"prBody\""),
            "ComposePrBodyReport JSON must not contain a \"prBody\" key; got: {json}"
        );
    }
}

/// Report output from `callisto add --format json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AddReport {
    pub schema_version: u32,
    /// Workspace-relative path of the changeset file written, or that would
    /// be written under `--dry-run`.
    pub path: String,
    /// The changeset's Markdown content. Present only under `--dry-run`,
    /// where nothing is written to disk and this is the only way to see it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl Report for AddReport {
    const COMMAND: &'static str = "add";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

/// Report output from `callisto pre enter`/`callisto pre exit --format json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreReport {
    pub schema_version: u32,
    /// `"pre"` after `enter`, `"exit"` after `exit`.
    pub mode: String,
    pub tag: String,
    /// Workspace-relative path of `.changeset/pre.json`. Present only under
    /// `--dry-run`, alongside `content`, since a real run's path is already
    /// implied by the fixed `.changeset/pre.json` location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl Report for PreReport {
    const COMMAND: &'static str = "pre";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

/// Init report output from `callisto init --format json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitReport {
    pub schema_version: u32,
    /// `true` when `callisto.toml` was written; `false` under `--dry-run`.
    pub initialized: bool,
    pub config_path: PathBuf,
    /// The `callisto.toml` content written, or that would be written.
    pub config: String,
    /// Every file init wrote, or would write under `--dry-run`: `config_path`, `.changeset/README.md`
    /// unless it already existed, and the release workflow when one was generated.
    pub files: Vec<PathBuf>,

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
