use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// §5.1, §6.1. A changeset's declared severity for one named package, **and** §7.4's internal
/// cascade outcome for an out-of-range dev-dependency ("spec rewrite only, no version bump").
/// Only the file-format usage is ever persisted to disk as a changeset.
///
/// **Variant order is deliberate, not alphabetical.** The derived `Ord` is the
/// aggregation-by-max lattice §7.1 relies on: `None < Patch < Minor < Major`.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    None,
    Patch,
    Minor,
    Major,
}

impl Severity {
    /// All four variants in ascending order — for exhaustive fixture tables and CLI help
    /// text, so no second hand-maintained list can drift from the enum.
    pub const ALL: [Severity; 4] = [
        Severity::None,
        Severity::Patch,
        Severity::Minor,
        Severity::Major,
    ];
}

/// §6.1: "case-insensitive read, lowercase write." `FromStr` is the read half, `Display` the
/// write half. The asymmetry is the spec, not a bug to unify.
impl std::str::FromStr for Severity {
    type Err = SeverityParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "major" => Ok(Severity::Major),
            "minor" => Ok(Severity::Minor),
            "patch" => Ok(Severity::Patch),
            "none" => Ok(Severity::None),
            _ => Err(SeverityParseError {
                found: s.to_string(),
            }),
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Severity::Major => "major",
            Severity::Minor => "minor",
            Severity::Patch => "patch",
            Severity::None => "none",
        })
    }
}

/// The token read where `major | minor | patch | none` (any case) was expected.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid severity {found:?}: expected one of \"major\", \"minor\", \"patch\", \"none\" (case-insensitive)")]
pub struct SeverityParseError {
    pub found: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn parses_lowercase_variants() {
        assert_eq!(Severity::from_str("major").unwrap(), Severity::Major);
        assert_eq!(Severity::from_str("minor").unwrap(), Severity::Minor);
        assert_eq!(Severity::from_str("patch").unwrap(), Severity::Patch);
        assert_eq!(Severity::from_str("none").unwrap(), Severity::None);
    }

    #[test]
    fn parses_case_insensitively() {
        assert_eq!(Severity::from_str("MAJOR").unwrap(), Severity::Major);
        assert_eq!(Severity::from_str("Minor").unwrap(), Severity::Minor);
    }

    #[test]
    fn rejects_unknown_token() {
        let err = Severity::from_str("critical").unwrap_err();
        assert_eq!(err.found, "critical");
    }

    #[test]
    fn displays_lowercase_regardless_of_input_case() {
        assert_eq!(Severity::Major.to_string(), "major");
        assert_eq!(Severity::None.to_string(), "none");
    }

    #[test]
    fn orders_none_patch_minor_major_ascending() {
        assert!(Severity::None < Severity::Patch);
        assert!(Severity::Patch < Severity::Minor);
        assert!(Severity::Minor < Severity::Major);
    }

    #[test]
    fn max_of_mixed_severities_picks_highest() {
        let severities = [
            Severity::Patch,
            Severity::None,
            Severity::Major,
            Severity::Minor,
        ];
        assert_eq!(severities.iter().copied().max().unwrap(), Severity::Major);
    }
}
