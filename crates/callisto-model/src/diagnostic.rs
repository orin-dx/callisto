use std::borrow::Cow;
use std::fmt;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::PackageId;

/// A configuration key in callisto.toml.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[schemars(with = "String")]
#[serde(transparent)]
pub struct ConfigKey(Cow<'static, str>);

impl ConfigKey {
    pub const CASCADE_MODE: Self = Self(Cow::Borrowed("cascade.mode"));
    pub const CASCADE_BUMP_SEVERITY: Self = Self(Cow::Borrowed("cascade.bump-severity"));
    pub const CASCADE_PEER_ESCALATION: Self = Self(Cow::Borrowed("cascade.peer-escalation"));
    pub const CASCADE_PRESERVE_NPM_RANGES: Self =
        Self(Cow::Borrowed("cascade.preserve-npm-ranges"));
    pub const VALIDATION_ALLOW_EMPTY_CHANGESETS: Self =
        Self(Cow::Borrowed("validation.allow-empty-changesets"));
    pub const RELEASE_TRIGGER: Self = Self(Cow::Borrowed("release-trigger"));
    pub const PRE_MAJOR_INFERENCE: Self = Self(Cow::Borrowed("pre-major-inference"));
    pub const TAG_TEMPLATE: Self = Self(Cow::Borrowed("tag-template"));
    pub const FIXED_GROUP: Self = Self(Cow::Borrowed("fixed-group"));
    pub const LINKED_GROUP: Self = Self(Cow::Borrowed("linked-group"));

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub const ALL: &'static [ConfigKey] = &[
        Self::CASCADE_MODE,
        Self::CASCADE_BUMP_SEVERITY,
        Self::CASCADE_PEER_ESCALATION,
        Self::CASCADE_PRESERVE_NPM_RANGES,
        Self::VALIDATION_ALLOW_EMPTY_CHANGESETS,
        Self::RELEASE_TRIGGER,
        Self::PRE_MAJOR_INFERENCE,
        Self::TAG_TEMPLATE,
        Self::FIXED_GROUP,
        Self::LINKED_GROUP,
    ];
}

impl fmt::Display for ConfigKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Machine-readable diagnostic warning or error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub severity: DiagnosticSeverity,
    pub message: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<PackageId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalated_by: Option<StrictFlag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governed_by: Option<ConfigKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum StrictFlag {
    Strict,
    StrictGraph,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum DiagnosticCode {
    EmptyChangeset,
    UnknownPackage,
    NapiTargetAddedNotInMembers,
    NapiTargetRemovedStillOnDisk,
    NapiCoordinationNotYetSupported,
    GraphEdgeDisagreement,
    RangeNotRoundTrippable,
    CatalogSpecNotRewritten,
    TagGlobNonVersionMatch,
    ChangesetsConfigKeyDropped,
    PreMajorInferenceInert,
    ChangelogSectionNotFound,
}
