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

/// Stable machine-readable identifier for one kind of [`Diagnostic`]. Serializes to
/// kebab-case (e.g. `"empty-changeset"`) — the wire form every `--format json` consumer
/// matches on, so a variant's Rust name and its JSON string always move together.
///
/// `#[non_exhaustive]`: a new variant is not a breaking change for a consumer that already
/// handles an unrecognized code gracefully, which any `--format json` consumer parsing this
/// as an open string set should.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum DiagnosticCode {
    /// A changeset file has no entries at all.
    EmptyChangeset,
    /// A changeset has one or more entries but an empty (or whitespace-only) summary.
    EmptySummary,
    /// A changeset entry names a package that isn't in the workspace (e.g. removed since the
    /// changeset was written) — the changeset stays on disk rather than being silently
    /// consumed, since deleting it would erase a still-unresolved entry.
    UnknownPackage,
    /// A changeset entry's package name does not parse as a valid `PackageId` at all.
    InvalidPackageName,
    /// A fixed napi group has a triple in `napi.targets` with no corresponding group member
    /// manifest — accept it by running `callisto init`.
    NapiTargetAddedNotInMembers,
    /// A fixed napi group member's manifest still exists on disk after its triple was removed
    /// from `napi.targets` — reconcile by running `callisto init`.
    NapiTargetRemovedStillOnDisk,
    /// Reserved for a pre-v0.3 milestone gate on napi platform coordination; not emitted by
    /// any current code path now that coordination has shipped.
    NapiCoordinationNotYetSupported,
    /// moon's declared project-graph edges and callisto's own manifest-derived dependency
    /// graph disagree about whether an edge exists, in either direction.
    GraphEdgeDisagreement,
    /// A dependency range spec's coverage of the new version is known (it covers or doesn't),
    /// but the mechanical rewrite of that range string toward the new version failed — e.g. a
    /// compound, wildcard, or prerelease-clause range no ecosystem's round-trip rewriter
    /// handles. The range is left alone and warned about rather than replaced with something
    /// that might not preserve the author's original intent.
    RangeNotRoundTrippable,
    /// A `DepSpec::Catalog` entry's coverage of the new version is never tested at all —
    /// catalog specs are never rewritten regardless of coverage (§13 inv. 15) — so this is
    /// reported under its own code rather than [`Self::RangeNotRoundTrippable`], whose
    /// trigger (a failed rewrite attempt) never applies to a catalog entry in the first place.
    CatalogSpecNotRewritten,
    /// A tag matched a `tag_template`-derived glob, but the substring at the `{version}`
    /// placeholder position did not parse as a valid version under the expected grammar.
    TagGlobNonVersionMatch,
    /// Reserved for `init`'s `.changeset/config.json` translation (§18 Q4, v0.4) reporting a
    /// config key from `@changesets/cli` that has no callisto equivalent and was dropped; not
    /// emitted by any current code path.
    ChangesetsConfigKeyDropped,
    /// Commit-severity inference failed for a package (e.g. a `git log` error) — the package's
    /// inferred severity is treated as absent rather than the run failing outright.
    PreMajorInferenceInert,
    /// Reserved for `plan-publish` reporting that it could not locate a release's section in
    /// its package's generated `CHANGELOG.md` (v0.2); not emitted by any current code path.
    ChangelogSectionNotFound,
    /// `plan-publish` could not compute the version plan it needs to determine which packages
    /// are ready to publish (a changeset/graph error surfaced as a soft diagnostic rather than
    /// aborting the whole publish-plan command).
    ChangesetReadError,
    /// Git repository or tag discovery failed while building a publish plan — release entries
    /// are omitted (HEAD SHA) or every package is treated as a release candidate (tag index)
    /// rather than the command failing outright.
    GitDiscoveryFailed,
    /// A bare (unprefixed) `[[package]]` config rule matched packages in two or more
    /// ecosystems — almost always unintended; use an ecosystem-prefixed pattern instead.
    BareRuleMatchesMultipleEcosystems,
    /// `callisto matrix` found a platform triple in `napi.targets`/`[tool.maturin].targets`
    /// that neither the shared triple-to-role table nor matrix's own host-runner table
    /// recognizes — excluded from the report's `targets[]` rather than silently guessing.
    UnrecognisedPlatformTriple,
    /// A package configures `publish-to` for a registry kind with no implemented dispatch
    /// (e.g. NuGet, GitHub Release) — the target is reported but not published.
    PublishTargetNotImplemented,
    /// A `[[package-set]]` config rule matched no packages after the workspace walk —
    /// advisory rather than a hard error, since a monorepo-wide rule can legitimately match
    /// nothing against a partial checkout or filtered walk.
    PackageSetMatchedNothing,
    /// `callisto matrix` found the same platform triple declared more than once within one
    /// package's own `napi.targets`/`[tool.maturin].targets` — the duplicate is dropped
    /// (first occurrence wins) rather than treated as a hard error.
    DuplicatePlatformTriple,
}

#[cfg(test)]
mod tests {
    use super::DiagnosticCode;

    /// AC-10: DiagnosticCode::BareRuleMatchesMultipleEcosystems must serialize
    /// to the kebab-case string "bare-rule-matches-multiple-ecosystems" and
    /// deserialize back to the same variant. The existing
    /// `#[serde(rename_all = "kebab-case")]` on DiagnosticCode handles this
    /// automatically — no per-variant annotation is needed.
    #[test]
    fn bare_rule_matches_multiple_ecosystems_serializes_to_kebab_case() {
        let code = DiagnosticCode::BareRuleMatchesMultipleEcosystems;
        let json = serde_json::to_string(&code).unwrap();
        assert_eq!(
            json,
            r#""bare-rule-matches-multiple-ecosystems""#,
            "DiagnosticCode::BareRuleMatchesMultipleEcosystems must serialize to \"bare-rule-matches-multiple-ecosystems\"",
        );
        let roundtrip: DiagnosticCode = serde_json::from_str(&json).unwrap();
        assert_eq!(
            roundtrip, code,
            "deserialized value must equal the original variant",
        );
    }

    /// AC-018: DiagnosticCode::UnrecognisedPlatformTriple must serialize to
    /// the kebab-case string "unrecognised-platform-triple" and deserialize
    /// back to the same variant.
    #[test]
    fn unrecognised_platform_triple_serializes_to_kebab_case() {
        let code = DiagnosticCode::UnrecognisedPlatformTriple;
        let json = serde_json::to_string(&code).unwrap();
        assert_eq!(
            json,
            r#""unrecognised-platform-triple""#,
            "DiagnosticCode::UnrecognisedPlatformTriple must serialize to \"unrecognised-platform-triple\"",
        );
        let roundtrip: DiagnosticCode = serde_json::from_str(&json).unwrap();
        assert_eq!(
            roundtrip, code,
            "deserialized value must equal the original variant",
        );
    }
}
