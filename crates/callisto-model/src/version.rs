use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Ecosystem;

/// §7.7. `SemVer` is the only grammar with an implementation in the committed v0.1–v0.4
/// scope; the rest are declared so `Ecosystem::version_grammar` is total.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
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
                let parsed = raw.parse::<pep440_rs::Version>().map_err(|e| VersionParseError {
                    raw: raw.to_string(),
                    grammar,
                    message: e.to_string(),
                })?;
                // PEP 440 defines multiple equivalent spellings for the same
                // version (e.g. `1.0.0-alpha1`, `1.0.0_alpha1`, `1.0.0a1`).
                // Normalize `raw` to pep440_rs's canonical rendering so that
                // logically-equal inputs produce identical `raw` values, and
                // therefore compare `==` and hash equal (derived PartialEq/Eq/
                // Hash include `raw`). SemVer has no analogous normalization
                // requirement (each version already has one canonical form),
                // so the caller's literal input is preserved for that grammar.
                let canonical = parsed.to_string();
                Ok(Version {
                    grammar,
                    raw: canonical,
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
            ParsedVersion::Pep440(v) => !v.is_post() && (v.is_pre() || v.is_dev()),
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
    /// Generic (grammar-unaware) deserialization. The serialized form of a
    /// `Version` is just its raw string (see `Serialize`), which does not
    /// carry the grammar it was parsed under, so this impl cannot *know*
    /// which grammar to use — it can only guess by trying grammars in turn,
    /// exactly as `VersionReq::deserialize` already does for Cargo/Npm/Pypi.
    ///
    /// SemVer is tried first because it is the strictest, most unambiguous
    /// grammar; PEP 440 is tried only if SemVer parsing fails. This means a
    /// string that happens to be valid under *both* grammars (e.g. plain
    /// `1.2.3`) always resolves to `VersionGrammar::SemVer`, never
    /// `VersionGrammar::Pep440` — the same residual-ambiguity tradeoff
    /// `VersionReq::deserialize` accepts for its Cargo/Npm/Pypi chain.
    ///
    /// Callers that know the intended ecosystem/grammar ahead of time (e.g.
    /// because a sibling field names it, or the value came from a
    /// known-SemVer-only source such as a git tag) should prefer
    /// [`Version::parse`] with an explicit [`VersionGrammar`] instead of
    /// going through this generic `serde` impl.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Version::parse(&s, VersionGrammar::SemVer)
            .or_else(|_| Version::parse(&s, VersionGrammar::Pep440))
            .map_err(serde::de::Error::custom)
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
                let req = raw
                    .parse::<pep440_rs::VersionSpecifiers>()
                    .map_err(|e| VersionParseError {
                        raw: raw.to_string(),
                        grammar,
                        message: e.to_string(),
                    })?;
                // Normalize raw to pep440_rs's canonical rendering so that
                // logically-equal specifiers (e.g. ">=1.0.0A1" vs ">=1.0.0a1")
                // produce identical raw values and therefore compare == and hash
                // equal (derived PartialEq/Eq/Hash include raw).
                let canonical = req.to_string();
                Ok(VersionReq {
                    grammar,
                    ecosystem,
                    req: ParsedVersionReq::Pep440(req),
                    raw: canonical,
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
    code(E029),
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
    code(E034),
    help("All version comparisons in a cascade step must share the same version grammar.")
)]
pub struct GrammarMismatch {
    pub left: VersionGrammar,
    pub right: VersionGrammar,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::Hash as _;

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

    #[test]
    fn pep440_dev_release_is_prerelease() {
        // PEP 440 dev releases (.devN) are pre-release in the sense that they
        // precede the final release and must be finalized before shipping.
        // is_prerelease() must return true for them, not just for alpha/beta/rc.
        let v = Version::parse("1.0.0.dev1", VersionGrammar::Pep440).unwrap();
        assert!(v.is_prerelease(), "1.0.0.dev1 must be considered a pre-release");

        let v2 = Version::parse("2.0.0.dev0", VersionGrammar::Pep440).unwrap();
        assert!(v2.is_prerelease(), "2.0.0.dev0 must be considered a pre-release");
    }

    #[test]
    fn pep440_non_canonical_inputs_normalize_to_equal_versions() {
        let dash = Version::parse("1.0.0-alpha1", VersionGrammar::Pep440).unwrap();
        let underscore = Version::parse("1.0.0_alpha1", VersionGrammar::Pep440).unwrap();
        let canonical = Version::parse("1.0.0a1", VersionGrammar::Pep440).unwrap();

        assert_eq!(dash, canonical);
        assert_eq!(underscore, canonical);

        let mut hasher_dash = std::collections::hash_map::DefaultHasher::new();
        dash.hash(&mut hasher_dash);
        let mut hasher_canonical = std::collections::hash_map::DefaultHasher::new();
        canonical.hash(&mut hasher_canonical);
        assert_eq!(
            std::hash::Hasher::finish(&hasher_dash),
            std::hash::Hasher::finish(&hasher_canonical)
        );
    }

    /// Gap 6: a genuinely malformed PEP 440 string returns a proper `Err`
    /// from the public parse entry point, never a panic.
    #[test]
    fn pep440_parse_malformed_string_returns_err_not_panic() {
        let result = Version::parse("garbage-not-a-version", VersionGrammar::Pep440);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.grammar, VersionGrammar::Pep440);
        assert_eq!(err.raw, "garbage-not-a-version");
    }

    /// Gap 7: PEP 440 pre-release markers are case-insensitive per the spec
    /// (`pep440_rs` normalizes both spellings to the same canonical `a1`
    /// form), so `A1` and `a1` parse to equal, identically-rendered versions.
    /// Verified empirically, not assumed.
    #[test]
    fn pep440_prerelease_marker_is_case_insensitive() {
        let upper = Version::parse("1.0.0A1", VersionGrammar::Pep440).unwrap();
        let lower = Version::parse("1.0.0a1", VersionGrammar::Pep440).unwrap();
        assert_eq!(upper, lower);
        assert_eq!(upper.render(), "1.0.0a1");
        assert_eq!(lower.render(), "1.0.0a1");
    }

    /// Gap 8: whitespace-padded input. Verified empirically: `pep440_rs`
    /// trims surrounding whitespace and accepts the input, normalizing `raw`
    /// to the trimmed canonical form; `semver` does not trim and rejects
    /// whitespace-padded input with a parse error. The two grammars behave
    /// differently here, so both are pinned explicitly.
    #[test]
    fn pep440_whitespace_padded_input_is_trimmed_and_accepted() {
        let v = Version::parse(" 1.0.0a1 ", VersionGrammar::Pep440).unwrap();
        assert_eq!(v.render(), "1.0.0a1");
    }

    #[test]
    fn semver_whitespace_padded_input_is_rejected() {
        let result = Version::parse(" 1.0.0 ", VersionGrammar::SemVer);
        assert!(result.is_err());
    }

    /// Gap 9 (RESOLVED): `Version::deserialize` now mirrors
    /// `VersionReq::deserialize`'s multi-grammar fallback (§ see doc comment
    /// on the `Deserialize` impl): SemVer is tried first since it is the
    /// strictest, most unambiguous grammar, and PEP 440 is tried only if
    /// SemVer parsing fails. A PEP-440-only version string (e.g. `1.2.3a1`,
    /// which SemVer rejects because of the bare `a1` suffix) now round-trips
    /// through `Version`'s serde impls and is recovered with
    /// `VersionGrammar::Pep440`.
    #[test]
    fn version_deserialize_falls_back_to_pep440_for_pep440_only_strings() {
        // Valid under PEP 440, but not valid SemVer.
        let pep440_only = Version::parse("1.2.3a1", VersionGrammar::Pep440).unwrap();
        assert_eq!(pep440_only.grammar(), VersionGrammar::Pep440);

        let json = serde_json::to_string(&pep440_only).unwrap();
        assert_eq!(json, "\"1.2.3a1\"");

        let round_tripped: Version = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped, pep440_only);
        assert_eq!(round_tripped.grammar(), VersionGrammar::Pep440);
    }

    #[test]
    fn pep440_post_dev_version_is_not_prerelease() {
        // PEP 440 orders 1.2.3.post1.dev1 ABOVE 1.2.3 (it is a dev build of a
        // post-release, not a pre-release of 1.2.3). is_prerelease() returning
        // true here causes bump() to finalize-in-place to 1.2.3, which is lower
        // than the input — wrong direction.
        let v = Version::parse("1.2.3.post1.dev1", VersionGrammar::Pep440).unwrap();
        assert!(
            !v.is_prerelease(),
            "1.2.3.post1.dev1 is above 1.2.3 in PEP 440 and must not be a pre-release"
        );
    }

    #[test]
    fn pep440_version_req_non_canonical_normalizes_for_eq() {
        // VersionReq::parse stores raw: raw.to_string() (un-normalized) in the
        // Pep440 arm. Two equivalent specifiers written differently hash/eq
        // differently, causing silent dedup failures in callers that use
        // VersionReq as a map key.
        let upper = VersionReq::parse(">=1.0.0A1", Ecosystem::Pypi).unwrap();
        let lower = VersionReq::parse(">=1.0.0a1", Ecosystem::Pypi).unwrap();
        assert_eq!(
            upper, lower,
            ">=1.0.0A1 and >=1.0.0a1 are the same PEP 440 specifier and must be equal"
        );
    }

    /// A string that a genuinely malformed value under both grammars still
    /// produces a clear parse error, not a panic, and the error surfaces
    /// after both attempts have failed.
    #[test]
    fn version_deserialize_rejects_strings_invalid_under_both_grammars() {
        let json = "\"not-a-version-at-all!!!\"";
        let result: Result<Version, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    /// Residual ambiguity, documented on the `Deserialize` impl: a string
    /// that parses successfully under both grammars (e.g. plain
    /// `major.minor.patch`) always resolves to `VersionGrammar::SemVer`,
    /// since SemVer is tried first. This mirrors the same tradeoff already
    /// accepted by `VersionReq::deserialize`'s Cargo/Npm/Pypi fallback chain.
    #[test]
    fn version_deserialize_prefers_semver_when_string_is_valid_under_both_grammars() {
        let json = "\"1.2.3\"";
        let v: Version = serde_json::from_str(json).unwrap();
        assert_eq!(v.grammar(), VersionGrammar::SemVer);
    }
}
