use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Ecosystem;

/// §7.7. `SemVer` is the only grammar with an implementation in the committed v0.1–v0.4
/// scope; the rest are declared so `Ecosystem::version_grammar` is total.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum VersionGrammar {
    SemVer,
    /// PEP 440 (§7.7) — declared, not implemented.
    Pep440,
    /// Maven's qualifier-ordering comparator (§7.7) — declared, not implemented.
    Maven,
}

/// The parsed form, kept alongside `raw` so comparison and component access are cheap.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ParsedVersion {
    SemVer(semver::Version),
    Pep440(pep440_rs::Version),
}

/// A parsed version, tagged with the grammar it was parsed under. §7.7, P4.
#[derive(Clone, Debug, PartialEq, Eq, Hash, JsonSchema)]
#[schemars(with = "String")]
pub struct Version {
    pub(crate) grammar: VersionGrammar,
    pub(crate) raw: String,
    #[schemars(skip)]
    pub(crate) parsed: ParsedVersion,
}

impl Version {
    pub fn parse(raw: &str, grammar: VersionGrammar) -> Result<Self, VersionParseError> {
        match grammar {
            VersionGrammar::SemVer => {
                let parsed = semver::Version::parse(raw).map_err(|e| VersionParseError {
                    raw: raw.to_string(),
                    grammar,
                    message: e.to_string(),
                })?;
                Ok(Version {
                    grammar,
                    raw: raw.to_string(),
                    parsed: ParsedVersion::SemVer(parsed),
                })
            }
            VersionGrammar::Pep440 => {
                let parsed = raw
                    .parse::<pep440_rs::Version>()
                    .map_err(|e| VersionParseError {
                        raw: raw.to_string(),
                        grammar,
                        message: e.to_string(),
                    })?;
                Ok(Version {
                    grammar,
                    raw: raw.to_string(),
                    parsed: ParsedVersion::Pep440(parsed),
                })
            }
            VersionGrammar::Maven => Err(VersionParseError {
                raw: raw.to_string(),
                grammar,
                message: format!("{grammar:?} has no versioning implementation yet (§7.7)"),
            }),
        }
    }

    pub fn semver(major: u64, minor: u64, patch: u64) -> Self {
        let parsed = semver::Version::new(major, minor, patch);
        Version {
            grammar: VersionGrammar::SemVer,
            raw: parsed.to_string(),
            parsed: ParsedVersion::SemVer(parsed),
        }
    }

    pub fn grammar(&self) -> VersionGrammar {
        self.grammar
    }

    pub fn render(&self) -> &str {
        &self.raw
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn major(&self) -> Option<u64> {
        match &self.parsed {
            ParsedVersion::SemVer(v) => Some(v.major),
            ParsedVersion::Pep440(v) => v.release().first().copied(),
        }
    }

    pub fn minor(&self) -> Option<u64> {
        match &self.parsed {
            ParsedVersion::SemVer(v) => Some(v.minor),
            ParsedVersion::Pep440(v) => v.release().get(1).copied(),
        }
    }

    pub fn patch(&self) -> Option<u64> {
        match &self.parsed {
            ParsedVersion::SemVer(v) => Some(v.patch),
            ParsedVersion::Pep440(v) => v.release().get(2).copied(),
        }
    }

    pub fn is_prerelease(&self) -> bool {
        match &self.parsed {
            ParsedVersion::SemVer(v) => !v.pre.is_empty(),
            ParsedVersion::Pep440(v) => v.is_pre(),
        }
    }

    pub fn compare(&self, other: &Version) -> Result<std::cmp::Ordering, GrammarMismatch> {
        if self.grammar != other.grammar {
            return Err(GrammarMismatch {
                left: self.grammar,
                right: other.grammar,
            });
        }
        match (&self.parsed, &other.parsed) {
            (ParsedVersion::SemVer(a), ParsedVersion::SemVer(b)) => Ok(a.cmp(b)),
            (ParsedVersion::Pep440(a), ParsedVersion::Pep440(b)) => Ok(a.cmp(b)),
            _ => unreachable!(),
        }
    }

    pub fn partial_compare(&self, other: &Version) -> Option<std::cmp::Ordering> {
        self.compare(other).ok()
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.raw)
    }
}

impl Serialize for Version {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.render())
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Version::parse(&s, VersionGrammar::SemVer).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ParsedVersionReq {
    SemVer(semver::VersionReq),
    Pep440(pep440_rs::VersionSpecifiers),
}

/// Parsed version requirement.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema)]
#[schemars(with = "String")]
pub struct VersionReq {
    grammar: VersionGrammar,
    ecosystem: Ecosystem,
    #[schemars(skip)]
    req: ParsedVersionReq,
    raw: String,
}

impl VersionReq {
    pub fn parse(raw: &str, ecosystem: Ecosystem) -> Result<Self, VersionParseError> {
        let grammar = ecosystem.version_grammar();
        match grammar {
            VersionGrammar::SemVer => {
                let req = semver::VersionReq::parse(raw).map_err(|e| VersionParseError {
                    raw: raw.to_string(),
                    grammar,
                    message: e.to_string(),
                })?;
                Ok(VersionReq {
                    grammar,
                    ecosystem,
                    req: ParsedVersionReq::SemVer(req),
                    raw: raw.to_string(),
                })
            }
            VersionGrammar::Pep440 => {
                let req =
                    raw.parse::<pep440_rs::VersionSpecifiers>()
                        .map_err(|e| VersionParseError {
                            raw: raw.to_string(),
                            grammar,
                            message: e.to_string(),
                        })?;
                Ok(VersionReq {
                    grammar,
                    ecosystem,
                    req: ParsedVersionReq::Pep440(req),
                    raw: raw.to_string(),
                })
            }
            VersionGrammar::Maven => Err(VersionParseError {
                raw: raw.to_string(),
                grammar,
                message: format!("{grammar:?} version requirements not implemented"),
            }),
        }
    }

    pub fn render(&self) -> &str {
        &self.raw
    }

    pub fn ecosystem(&self) -> Ecosystem {
        self.ecosystem
    }

    pub fn matches(&self, v: &Version) -> Result<bool, GrammarMismatch> {
        if self.grammar != v.grammar() {
            return Err(GrammarMismatch {
                left: self.grammar,
                right: v.grammar(),
            });
        }
        match (&self.req, &v.parsed) {
            (ParsedVersionReq::SemVer(req), ParsedVersion::SemVer(sv)) => Ok(req.matches(sv)),
            (ParsedVersionReq::Pep440(req), ParsedVersion::Pep440(pv)) => Ok(req.contains(pv)),
            _ => unreachable!(),
        }
    }
}

impl Serialize for VersionReq {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.render())
    }
}

impl<'de> Deserialize<'de> for VersionReq {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        VersionReq::parse(&s, Ecosystem::Cargo)
            .or_else(|_| VersionReq::parse(&s, Ecosystem::Npm))
            .or_else(|_| VersionReq::parse(&s, Ecosystem::Pypi))
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, miette::Diagnostic)]
#[error("`{raw}` is not a valid {grammar:?} version: {message}")]
#[diagnostic(
    code(E020),
    help("Ensure the version string strictly adheres to the {grammar:?} specification.")
)]
pub struct VersionParseError {
    pub raw: String,
    pub grammar: VersionGrammar,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, miette::Diagnostic)]
#[error("cannot compare a {left:?} version with a {right:?} version")]
#[diagnostic(
    code(E021),
    help("All version comparisons in a cascade step must share the same version grammar.")
)]
pub struct GrammarMismatch {
    pub left: VersionGrammar,
    pub right: VersionGrammar,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_semver_and_exposes_components() {
        let v = Version::parse("1.2.3", VersionGrammar::SemVer).unwrap();
        assert_eq!(v.grammar(), VersionGrammar::SemVer);
        assert_eq!(v.major(), Some(1));
        assert_eq!(v.minor(), Some(2));
        assert_eq!(v.patch(), Some(3));
        assert!(!v.is_prerelease());
    }

    #[test]
    fn serde_version_roundtrips() {
        let v = Version::parse("1.2.3", VersionGrammar::SemVer).unwrap();
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, "\"1.2.3\"");
        let deserialized: Version = serde_json::from_str(&json).unwrap();
        assert_eq!(v, deserialized);
    }

    #[test]
    fn parses_valid_pep440_and_prerelease() {
        let v = Version::parse("0.3.2a1", VersionGrammar::Pep440).unwrap();
        assert_eq!(v.grammar(), VersionGrammar::Pep440);
        assert_eq!(v.major(), Some(0));
        assert_eq!(v.minor(), Some(3));
        assert_eq!(v.patch(), Some(2));
        assert!(v.is_prerelease());

        let req = VersionReq::parse(">=0.3.0", Ecosystem::Pypi).unwrap();
        assert!(req.matches(&v).unwrap());
    }
}
