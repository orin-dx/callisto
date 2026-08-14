use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawConfig {
    pub changesets: Option<RawChangesetsConfig>,
    pub cascade: Option<RawCascadeConfig>,
    pub validation: Option<RawValidationConfig>,
    pub registries: Option<BTreeMap<String, RawRegistryConfig>>,
    pub package: Option<Vec<RawPackageConfig>>,
    #[serde(rename = "package-set")]
    pub package_set: Option<Vec<RawPackageSetConfig>>,
    #[serde(rename = "fixed-group")]
    pub fixed_group: Option<Vec<crate::config::groups::RawGroup>>,
    #[serde(rename = "linked-group")]
    pub linked_group: Option<Vec<crate::config::groups::RawGroup>>,
    /// `callisto init` bookkeeping (§18 Q5.4 mechanism 1), not a user-facing
    /// policy section — records the workspace state `init` last reconciled
    /// against, so a later run can diff the freshly-discovered state against
    /// it instead of against nothing.
    pub init: Option<RawInitConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawInitConfig {
    /// Ecosystem prefixes (`Ecosystem::prefix()`: `"cargo"`, `"npm"`, ...)
    /// present in the workspace as of the last `init` run that wrote or
    /// reconciled this file.
    pub ecosystems: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawChangesetsConfig {
    /// Workspace-root-relative. Always forward-slash-separated (`/`), never `\`, regardless
    /// of the OS this value was authored or is read on — `\` is a legal filename character
    /// on POSIX, not a separator, so a backslash-separated value would silently name a
    /// single oddly-named directory rather than a nested path the moment this file is read
    /// on a non-Windows host. Rejected if absolute or containing `..` components
    /// (`ConfigError::InvalidChangesetsDir`, E116).
    pub dir: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawCascadeConfig {
    pub mode: Option<String>,
    #[serde(rename = "bump-severity")]
    pub bump_severity: Option<String>,
    #[serde(rename = "peer-escalation")]
    pub peer_escalation: Option<bool>,
    #[serde(rename = "preserve-npm-ranges")]
    pub preserve_npm_ranges: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawValidationConfig {
    #[serde(rename = "allow-empty-changesets")]
    pub allow_empty_changesets: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawRegistryConfig {
    pub kind: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPackageConfig {
    #[serde(rename = "match")]
    pub pattern: String,
    #[serde(rename = "release-trigger")]
    pub release_trigger: Option<String>,
    #[serde(rename = "publish-to")]
    pub publish_to: Option<Vec<String>>,
    #[serde(rename = "tag-template")]
    pub tag_template: Option<String>,
    /// Package-root-relative. Always forward-slash-separated (`/`), never `\` — same
    /// portability rule as `[changesets].dir` (see its doc comment): this value is parsed
    /// through `workspace_relative`, which only normalizes the *host* platform's native
    /// separator, so a `\`-separated value written on Windows is misread as one literal
    /// path component the moment the same config is read on a non-Windows host. Rejected
    /// if absolute or containing `..` components (`ConfigError::InvalidChangelogPath`, E117).
    pub changelog: Option<String>,
    #[serde(rename = "pre-major-inference")]
    pub pre_major_inference: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPackageSetConfig {
    #[serde(rename = "match")]
    pub pattern: String,
    #[serde(rename = "release-trigger")]
    pub release_trigger: Option<String>,
    #[serde(rename = "publish-to")]
    pub publish_to: Option<Vec<String>>,
    #[serde(rename = "tag-template")]
    pub tag_template: Option<String>,
    /// Package-root-relative. Always forward-slash-separated (`/`), never `\` — same
    /// portability rule as `[changesets].dir` (see its doc comment): this value is parsed
    /// through `workspace_relative`, which only normalizes the *host* platform's native
    /// separator, so a `\`-separated value written on Windows is misread as one literal
    /// path component the moment the same config is read on a non-Windows host. Rejected
    /// if absolute or containing `..` components (`ConfigError::InvalidChangelogPath`, E117).
    pub changelog: Option<String>,
    #[serde(rename = "pre-major-inference")]
    pub pre_major_inference: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawMoonYml {
    pub extensions: Option<RawMoonExtensions>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawMoonExtensions {
    pub callisto: Option<RawMoonCallistoConfig>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawMoonCallistoConfig {
    #[serde(rename = "package-name")]
    pub package_name: Option<String>,
    #[serde(rename = "release-trigger")]
    pub release_trigger: Option<String>,
    #[serde(rename = "publish-to")]
    pub publish_to: Option<Vec<String>>,
    #[serde(rename = "tag-template")]
    pub tag_template: Option<String>,
    /// Package-root-relative; intended to follow the same forward-slash-only portability
    /// rule as `[changesets].dir` (see its doc comment) and to be rejected the same way if
    /// absolute or containing `..` components (`ConfigError::InvalidChangelogPath`, E117) —
    /// but nothing in this crate actually deserializes a `moon.yml` file into
    /// `RawMoonCallistoConfig` today, so none of that is enforced in practice; this
    /// documents the intended contract for whenever that wiring exists, not current behavior.
    pub changelog: Option<String>,
    #[serde(rename = "pre-major-inference")]
    pub pre_major_inference: Option<String>,
}
