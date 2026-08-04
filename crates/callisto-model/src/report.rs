use std::path::PathBuf;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{
    CommitSha, ConfigKey, DepKind, Diagnostic, Ecosystem, GroupName, PackageId, PublishPlan,
    Severity, TagName, Version,
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

/// Publish execution report output from `callisto publish --format json`
/// (non-dry-run only). Distinct from [`PublishPlan`], which describes what
/// *would* be published (used both by `plan-publish` and by `publish
/// --dry-run`); [`PublishReport`] instead records what actually happened for
/// every package `publish` attempted to send to its registry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublishReport {
    pub schema_version: u32,
    pub attempts: Vec<PublishAttempt>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl Report for PublishReport {
    const COMMAND: &'static str = "publish";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

/// The outcome of one package's actual publish attempt, as recorded in a
/// [`PublishReport`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublishAttempt {
    pub package: PackageId,
    pub version: Version,
    #[serde(flatten)]
    pub result: PublishAttemptResult,
}

/// Per-package result of a real (non-dry-run) publish attempt. Mirrors
/// [`crate::PublishOutcome`] for the success cases, plus a `Failed` case
/// carrying the registry error's message for packages that could not be
/// published.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum PublishAttemptResult {
    /// The package/version was newly uploaded to the registry.
    Published,
    /// The package/version was already present on the registry.
    AlreadyPublished,
    /// The publish attempt failed; `error` is the registry error's message.
    Failed { error: String },
}

impl PublishAttemptResult {
    /// Returns `true` if this result represents a failure.
    pub fn is_failure(&self) -> bool {
        matches!(self, PublishAttemptResult::Failed { .. })
    }
}

impl PublishReport {
    /// Returns `true` if any attempt in this report resulted in a failure.
    pub fn has_failures(&self) -> bool {
        self.attempts.iter().any(|a| a.result.is_failure())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PackageId, Version, VersionGrammar};

    fn pkg() -> PackageId {
        PackageId::parse("test-pkg").unwrap()
    }

    fn ver() -> Version {
        Version::parse("1.0.0", VersionGrammar::SemVer).unwrap()
    }

    fn attempt(result: PublishAttemptResult) -> PublishAttempt {
        PublishAttempt {
            package: pkg(),
            version: ver(),
            result,
        }
    }

    fn report(attempts: Vec<PublishAttempt>) -> PublishReport {
        PublishReport {
            schema_version: SCHEMA_VERSION,
            attempts,
            diagnostics: vec![],
        }
    }

    #[test]
    fn has_failures_returns_true_when_any_attempt_failed() {
        let r = report(vec![
            attempt(PublishAttemptResult::Published),
            attempt(PublishAttemptResult::Failed {
                error: "registry unavailable".to_string(),
            }),
        ]);
        assert!(r.has_failures());
    }

    #[test]
    fn has_failures_returns_true_when_all_attempts_failed() {
        let r = report(vec![
            attempt(PublishAttemptResult::Failed {
                error: "auth error".to_string(),
            }),
            attempt(PublishAttemptResult::Failed {
                error: "network error".to_string(),
            }),
        ]);
        assert!(r.has_failures());
    }

    #[test]
    fn has_failures_returns_false_when_all_attempts_succeeded() {
        let r = report(vec![
            attempt(PublishAttemptResult::Published),
            attempt(PublishAttemptResult::AlreadyPublished),
        ]);
        assert!(!r.has_failures());
    }

    #[test]
    fn has_failures_returns_false_for_empty_report() {
        let r = report(vec![]);
        assert!(!r.has_failures());
    }

    #[test]
    fn is_failure_is_true_only_for_failed_variant() {
        assert!(PublishAttemptResult::Failed {
            error: "oops".to_string()
        }
        .is_failure());
        assert!(!PublishAttemptResult::Published.is_failure());
        assert!(!PublishAttemptResult::AlreadyPublished.is_failure());
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
    /// `true` only on a first run, when `callisto.toml` did not exist yet and
    /// was written directly. `false` on every re-run (§18 Q5.4 mechanism 1),
    /// including a re-run that applies detected drift — that case is
    /// reported through `diff`, not this flag.
    pub initialized: bool,
    pub config_path: PathBuf,
    /// Drift between the currently-discovered workspace state and what is
    /// already recorded in `callisto.toml`, and whether that drift was
    /// applied this run (docs/00-design.md §18 Q5.4 mechanism 1: re-running
    /// `init` is the reconcile flow — it re-detects, reports a diff, and
    /// applies only with confirmation). Empty/`applied: false` when there is
    /// nothing to reconcile, including on a first run.
    #[serde(default)]
    pub diff: InitDiff,

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

/// The reconcile diff computed by a `callisto init` re-run (§18 Q5.4
/// mechanism 1). Carries *what would change*, not just whether something
/// changed, so a wrapper (CLI text renderer, `--format json` consumer) can
/// narrate the drift instead of a bare boolean.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitDiff {
    /// Ecosystems present in the discovered workspace but not yet recorded
    /// against the existing `callisto.toml` (e.g. a `package.json` added to
    /// a previously Cargo-only workspace, or `napi.targets` appearing).
    /// Sorted for determinism. Empty when there is no drift to reconcile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub new_ecosystems: Vec<Ecosystem>,
    /// `true` when `new_ecosystems` was non-empty and was written to
    /// `callisto.toml` this run (`InitOptions::yes`). `false` when the diff
    /// was only reported (dry-preview) or when there was no diff to apply.
    #[serde(default)]
    pub applied: bool,
}
