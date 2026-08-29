use std::path::PathBuf;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{
    CommitSha, ConfigKey, DepKind, Diagnostic, Ecosystem, GroupName, PackageId, PublishPlan, ReleaseTrigger, Severity,
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
    /// The publish attempt failed.
    ///
    /// `kind` is a camelCase discriminator matching the `RegistryError` variant
    /// ("rateLimited", "authFailed", "network", "other") so callers can
    /// programmatically distinguish error types without parsing `error`.
    /// `error` is the human-readable message.
    Failed {
        #[serde(rename = "errorKind")]
        kind: String,
        error: String,
    },
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
                kind: "other".to_string(),
                error: "registry unavailable".to_string(),
            }),
        ]);
        assert!(r.has_failures());
    }

    #[test]
    fn has_failures_returns_true_when_all_attempts_failed() {
        let r = report(vec![
            attempt(PublishAttemptResult::Failed {
                kind: "authFailed".to_string(),
                error: "auth error".to_string(),
            }),
            attempt(PublishAttemptResult::Failed {
                kind: "network".to_string(),
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
            kind: "other".to_string(),
            error: "oops".to_string()
        }
        .is_failure());
        assert!(!PublishAttemptResult::Published.is_failure());
        assert!(!PublishAttemptResult::AlreadyPublished.is_failure());
    }

    /// PUB-007: the JSON for a failed attempt must carry a machine-readable
    /// `errorKind` discriminator so callers can distinguish rate-limit failures
    /// from auth failures without parsing the human-readable `error` string.
    #[test]
    fn failed_attempt_json_contains_error_kind_discriminator() {
        let attempt = attempt(PublishAttemptResult::Failed {
            kind: "rateLimited".to_string(),
            error: "Rate limited. Retry after 60s".to_string(),
        });
        let json = serde_json::to_string(&attempt).unwrap();
        assert!(
            json.contains("\"errorKind\""),
            "failed attempt JSON must contain 'errorKind' field; got: {json}"
        );
        assert!(
            json.contains("\"rateLimited\""),
            "failed attempt JSON must carry the kind value; got: {json}"
        );
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
    /// Mandatory (§12.5) — the field the Action's mode dispatch reads.
    /// Always serialized, never omitted, even when `false`.
    pub has_changesets: bool,
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

    /// Mandatory, always serialized — §6.3's empty-changeset validation and
    /// §G.9.3's `changed_since_last_tag` depend on this being present even
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

    /// docs/01-spec.md §M.12.4: `StatusReport.hasChangesets` is mandatory —
    /// §12.5 mandates it and §12.2 branch 3 gates the Action's mode dispatch
    /// on it — so it must always be present in the serialized JSON, never
    /// omitted regardless of its value.
    #[test]
    fn status_report_json_always_contains_has_changesets() {
        let report = StatusReport {
            schema_version: SCHEMA_VERSION,
            has_changesets: false,
            packages: vec![],
            diagnostics: vec![],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(
            json.contains("\"hasChangesets\":false"),
            "StatusReport JSON must always contain hasChangesets, even when false; got: {json}"
        );
    }

    /// docs/01-spec.md §M.12.4: `StatusEntry.changedSinceLastTag` is
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

    /// docs/01-spec.md §M.12.4: `StatusEntry` carries `lastReleasedVersion`
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
    /// docs/01-spec.md §M.12.5: the composed PR body. Wire key is `body`
    /// (not `prBody`).
    ///
    /// NOTE: docs/01-spec.md §M.12.5 also documents a `metadata:
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

    /// docs/01-spec.md §M.12.5: `ComposePrBodyReport`'s composed-body field is
    /// documented as `body`, serialized camelCase as `"body"` -- not `"prBody"`.
    ///
    /// NOTE: intentionally doesn't assert a `metadata` key. §M.12.5 also
    /// documents `metadata: PrBodyMetadata` (labels/managedLabels/overflow),
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

/// Validate report output from `callisto validate --format json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidateReport {
    pub schema_version: u32,
    /// `false` iff any diagnostic has `severity == Error` (after
    /// `--strict`/`--strict-graph` escalation has been applied).
    pub ok: bool,

    /// Mandatory here, unlike every other report -- `validate`'s entire
    /// payload *is* its diagnostics, so an absent key would leave the
    /// command with nothing to say.
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

#[cfg(test)]
mod validate_report_tests {
    use super::*;

    /// docs/01-spec.md §M.12.6: `ValidateReport`'s boolean field is documented
    /// as `ok`, serialized camelCase as `"ok"` -- not `"valid"`.
    #[test]
    fn validate_report_json_uses_ok_key_not_valid() {
        let report = ValidateReport {
            schema_version: SCHEMA_VERSION,
            ok: true,
            diagnostics: vec![],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(
            json.contains("\"ok\":true"),
            "ValidateReport JSON must contain the \"ok\" key; got: {json}"
        );
        assert!(
            !json.contains("\"valid\""),
            "ValidateReport JSON must not contain a \"valid\" key; got: {json}"
        );
    }

    /// docs/01-spec.md §M.12.6: "Mandatory here, unlike every other report --
    /// validate's entire payload *is* its diagnostics, so an absent key would
    /// leave the command with nothing to say." `diagnostics` must always be
    /// serialized, even when empty, unlike every other report's diagnostics
    /// field.
    #[test]
    fn validate_report_json_always_contains_diagnostics_even_when_empty() {
        let report = ValidateReport {
            schema_version: SCHEMA_VERSION,
            ok: true,
            diagnostics: vec![],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(
            json.contains("\"diagnostics\":[]"),
            "ValidateReport JSON must always contain diagnostics, even when empty; got: {json}"
        );
    }
}

/// Tag report output from `callisto tag --format json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TagReport {
    pub schema_version: u32,
    pub tags: Vec<CreatedTag>,

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
    /// `true` when the tag already existed at this sha and was left alone --
    /// P3's idempotence made observable rather than assumed. An existing tag
    /// at a *different* sha is an error diagnostic, not a silent overwrite
    /// (docs/01-spec.md §M.12.6).
    pub already_existed: bool,
    /// `true` for a floating major-version alias (e.g. `v1`), which is
    /// intentionally force-moved to point at each new release; `false` for
    /// an immutable per-version release tag, which must never move once
    /// created. Callers that push tags to a remote (`callisto-action`) use
    /// this to scope `git push --force` to only the entries that actually
    /// need it, instead of force-pushing every tag indiscriminately.
    #[serde(default)]
    pub is_floating_major: bool,
}

#[cfg(test)]
mod tag_report_tests {
    use super::*;

    fn tag_name() -> TagName {
        TagName("pkg-a@1.0.0".to_string())
    }

    fn sha() -> CommitSha {
        CommitSha::parse("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0").unwrap()
    }

    fn pkg() -> PackageId {
        PackageId::parse("pkg-a").unwrap()
    }

    /// docs/01-spec.md §M.12.6: `TagReport`'s array field is documented as
    /// `tags: Vec<CreatedTag>`, serialized camelCase as `"tags"` -- not
    /// `"createdTags"`.
    #[test]
    fn tag_report_json_uses_tags_key_not_created_tags() {
        let report = TagReport {
            schema_version: SCHEMA_VERSION,
            tags: vec![CreatedTag {
                package: pkg(),
                tag_name: tag_name(),
                sha: sha(),
                already_existed: false,
                is_floating_major: false,
            }],
            diagnostics: vec![],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(
            json.contains("\"tags\":["),
            "TagReport JSON must contain the \"tags\" key; got: {json}"
        );
        assert!(
            !json.contains("\"createdTags\""),
            "TagReport JSON must not contain a \"createdTags\" key; got: {json}"
        );
    }

    /// docs/01-spec.md §M.12.6: `CreatedTag::already_existed` is `true` when
    /// the tag already existed at this sha and was left alone (P3's
    /// idempotence made observable). Serialized camelCase as
    /// `"alreadyExisted"`.
    #[test]
    fn created_tag_json_includes_already_existed_key() {
        let tag = CreatedTag {
            package: pkg(),
            tag_name: tag_name(),
            sha: sha(),
            already_existed: true,
            is_floating_major: false,
        };
        let json = serde_json::to_string(&tag).unwrap();
        assert!(
            json.contains("\"alreadyExisted\":true"),
            "CreatedTag JSON must contain the \"alreadyExisted\" key; got: {json}"
        );
    }

    /// `CreatedTag::is_floating_major` distinguishes a floating major-version
    /// alias from an immutable per-version release tag, serialized camelCase
    /// as `"isFloatingMajor"`. `#[serde(default)]` so a pre-existing tag
    /// report JSON without the field still deserializes (defaults to false).
    #[test]
    fn created_tag_json_includes_is_floating_major_key() {
        let tag = CreatedTag {
            package: pkg(),
            tag_name: tag_name(),
            sha: sha(),
            already_existed: false,
            is_floating_major: true,
        };
        let json = serde_json::to_string(&tag).unwrap();
        assert!(
            json.contains("\"isFloatingMajor\":true"),
            "CreatedTag JSON must contain the \"isFloatingMajor\" key; got: {json}"
        );
    }

    #[test]
    fn created_tag_deserializes_when_is_floating_major_is_absent() {
        let json = format!(
            r#"{{"package":"{}","tagName":"{}","sha":"{}","alreadyExisted":false}}"#,
            pkg(),
            tag_name().as_str(),
            sha().as_str(),
        );
        let tag: CreatedTag = serde_json::from_str(&json).expect("must deserialize without isFloatingMajor");
        assert!(
            !tag.is_floating_major,
            "is_floating_major must default to false when absent from JSON"
        );
    }
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
