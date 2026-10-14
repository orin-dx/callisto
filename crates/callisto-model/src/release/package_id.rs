use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize, Serializer};

use crate::identity::string_json_schema;
use crate::{Ecosystem, PackageId};

/// An exact, ecosystem-qualified package identity for durable release operations.
///
/// Unlike PackageId, this type has no bare form and therefore cannot use
/// wildcard matching across ecosystems.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct ReleasePackageId {
    ecosystem: Ecosystem,
    name: String,
}

string_json_schema!(ReleasePackageId);

impl ReleasePackageId {
    /// Creates an exact release package identity.
    ///
    /// # Errors
    ///
    /// Returns ReleasePackageIdParseError when name is not valid in the
    /// existing package-identity grammar.
    pub fn new(ecosystem: Ecosystem, name: impl AsRef<str>) -> Result<Self, ReleasePackageIdParseError> {
        let name = name.as_ref();
        if let Err(reason) = check_release_package_name(ecosystem, name) {
            return Err(ReleasePackageIdParseError::UnsafeName {
                raw: format!("{}/{}", ecosystem.prefix(), name),
                reason,
            });
        }
        let raw = format!("{}/{}", ecosystem.prefix(), name);
        match PackageId::parse(&raw) {
            Ok(PackageId::Prefixed {
                ecosystem: parsed_ecosystem,
                name: parsed_name,
            }) if parsed_ecosystem == ecosystem && parsed_name == name => Ok(Self {
                ecosystem,
                name: parsed_name,
            }),
            _ => Err(ReleasePackageIdParseError::Malformed { raw }),
        }
    }

    /// Parses the canonical 'ecosystem/name' release identity form.
    ///
    /// PackageId also accepts 'ecosystem:name'; release identities reject it
    /// so every durable encoding has exactly one spelling.
    pub fn parse(s: &str) -> Result<Self, ReleasePackageIdParseError> {
        if s.contains(':') {
            return Err(ReleasePackageIdParseError::NonCanonical { raw: s.to_string() });
        }
        let (prefix, name) = s
            .split_once('/')
            .ok_or_else(|| ReleasePackageIdParseError::MissingEcosystem { raw: s.to_string() })?;
        let ecosystem = Ecosystem::from_prefix(prefix)
            .ok_or_else(|| ReleasePackageIdParseError::UnknownEcosystem { raw: s.to_string() })?;
        let id = Self::new(ecosystem, name)?;
        if id.to_string() != s {
            return Err(ReleasePackageIdParseError::NonCanonical { raw: s.to_string() });
        }
        Ok(id)
    }

    /// Returns the exact ecosystem.
    pub fn ecosystem(&self) -> Ecosystem {
        self.ecosystem
    }

    /// Returns the package name without the ecosystem prefix.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for ReleasePackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.ecosystem.prefix(), self.name)
    }
}

impl FromStr for ReleasePackageId {
    type Err = ReleasePackageIdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl PartialOrd for ReleasePackageId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ReleasePackageId {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.ecosystem.prefix(), self.name.as_str()).cmp(&(other.ecosystem.prefix(), other.name.as_str()))
    }
}

impl Serialize for ReleasePackageId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl TryFrom<String> for ReleasePackageId {
    type Error = ReleasePackageIdParseError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::parse(&raw)
    }
}

/// Errors produced while parsing a ReleasePackageId.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ReleasePackageIdParseError {
    #[error("release package identity {raw} must include an ecosystem prefix")]
    MissingEcosystem { raw: String },
    #[error("release package identity {raw} has an unknown ecosystem prefix")]
    UnknownEcosystem { raw: String },
    #[error("release package identity {raw} is malformed")]
    Malformed { raw: String },
    #[error("release package identity {raw} is not in canonical ecosystem/name form")]
    NonCanonical { raw: String },
    #[error("release package identity {raw} is unsafe: {reason}")]
    UnsafeName { raw: String, reason: &'static str },
}

/// The charset every release package name must satisfy, per ecosystem.
///
/// A release package name is not just a label: it becomes an argv word
/// (`npm view <name>@<version>`, `pnpm publish --filter=<name>`) and a URL
/// path segment (the cargo sparse index, the PyPI JSON API). `TagName` and
/// `GitHubRepository` already rule out the same hazards for their own
/// identities; this is the equivalent rule for package names, applied where
/// [`ReleasePackageId`] is minted so no release path can skip it.
///
/// Each ecosystem's real grammar is enforced rather than a lowest common
/// denominator, because a name outside it cannot name a publishable package
/// anyway and a plan-time rejection is cheaper than a half-published release.
fn check_release_package_name(ecosystem: Ecosystem, name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("name is empty");
    }
    if name.len() > 214 {
        return Err("name is longer than any registry accepts");
    }
    if name.starts_with('-') {
        return Err("a leading `-` would be read as a command-line option");
    }
    if name.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("name contains a control character or whitespace");
    }
    // `/` is allowed only as the single separator of an npm `@scope/name`,
    // handled below; every other character here can change the meaning of a
    // URL path or an argv word.
    if name.contains(['?', '#', '%', '\\', ':', '@']) && !(ecosystem == Ecosystem::Npm && name.starts_with('@')) {
        return Err("name contains a character that can alter a URL path or argv word");
    }
    match ecosystem {
        Ecosystem::Cargo => check_cargo_name(name),
        Ecosystem::Npm => check_npm_name(name),
        Ecosystem::Pypi => check_pypi_name(name),
        // No publish path is implemented for these, so only the shared
        // argv/URL rules above apply; a `/` would still traverse a URL path.
        _ => {
            if name.contains('/') {
                Err("name contains a path separator")
            } else {
                Ok(())
            }
        }
    }
}

/// crates.io: ASCII alphanumerics, `-` and `_`, starting with a letter or `_`.
fn check_cargo_name(name: &str) -> Result<(), &'static str> {
    let first = name.chars().next().unwrap_or('\0');
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err("a cargo crate name must start with a letter or underscore");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err("a cargo crate name may only contain letters, digits, `-` and `_`");
    }
    Ok(())
}

/// npm: lowercase, URL-safe, with an optional single `@scope/` prefix.
fn check_npm_name(name: &str) -> Result<(), &'static str> {
    let (scoped, unscoped) = match name.strip_prefix('@') {
        Some(rest) => {
            let (scope, package) = rest
                .split_once('/')
                .ok_or("an npm scope must be followed by `/` and a package name")?;
            check_npm_segment(scope, true)?;
            (true, package)
        }
        None => (false, name),
    };
    check_npm_segment(unscoped, scoped)
}

/// npm allows legacy uppercase names and, only under a scope, a leading `.` or `_`.
fn check_npm_segment(segment: &str, scoped: bool) -> Result<(), &'static str> {
    let first = segment.chars().next().unwrap_or('\0');
    let leading_ok = first.is_ascii_alphanumeric() || (scoped && matches!(first, '.' | '_'));
    if !leading_ok {
        return Err(
            "an npm name segment must start with a letter or digit (a scoped name may also start with `.` or `_`)",
        );
    }
    if !segment
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err("an npm name segment may only contain letters, digits, `-`, `_` and `.`");
    }
    Ok(())
}

/// PEP 508: alphanumerics separated by single `-`, `_` or `.` runs, starting
/// and ending with an alphanumeric.
fn check_pypi_name(name: &str) -> Result<(), &'static str> {
    let boundary_ok = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    if !boundary_ok(name.chars().next()) || !boundary_ok(name.chars().last()) {
        return Err("a PyPI project name must start and end with a letter or digit");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err("a PyPI project name may only contain letters, digits, `-`, `_` and `.`");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::identity::assert_string_wire_schema;
    use crate::Ecosystem;

    use super::*;

    #[test]
    fn release_package_id_schema_describes_canonical_strings() {
        for raw in ["cargo/example", "npm/@scope/example", "pypi/example-package"] {
            let id = ReleasePackageId::parse(raw).unwrap();
            assert_string_wire_schema(&id, "ReleasePackageId", raw);
        }
    }

    #[test]
    fn release_package_id_requires_a_qualified_canonical_identity() {
        assert_eq!(
            ReleasePackageId::parse("cargo/shared").unwrap().to_string(),
            "cargo/shared"
        );
        assert_eq!(ReleasePackageId::parse("npm/shared").unwrap().to_string(), "npm/shared");
        assert!(ReleasePackageId::parse("shared").is_err());
        assert!(ReleasePackageId::parse("cargo:shared").is_err());
        assert!(ReleasePackageId::parse("unknown/shared").is_err());
        assert!(ReleasePackageId::parse("cargo/../shared").is_err());
        assert!(ReleasePackageId::new(Ecosystem::Cargo, "").is_err());
    }

    #[test]
    fn release_package_ids_do_not_conflate_ecosystems() {
        let cargo = ReleasePackageId::parse("cargo/shared").unwrap();
        let npm = ReleasePackageId::parse("npm/shared").unwrap();
        assert_ne!(cargo, npm);
        assert!(cargo < npm);
    }

    #[test]
    fn release_package_id_deserialization_rejects_a_bare_identity() {
        let parsed = serde_json::from_str::<ReleasePackageId>(r#""shared""#);
        assert!(parsed.is_err());
    }

    /// A release package name becomes an argv word (`npm view <name>@<v>`,
    /// `pnpm publish --filter=<name>`) and a URL path segment (the cargo
    /// sparse index, the PyPI JSON API). These are the shapes that would
    /// change the meaning of one of those, so they are refused at plan time.
    #[test]
    fn release_package_names_reject_argv_and_url_hazards() {
        let hazards = [
            (Ecosystem::Cargo, "-x"),
            (Ecosystem::Cargo, "--registry"),
            (Ecosystem::Cargo, "a b"),
            (Ecosystem::Cargo, "a\nb"),
            (Ecosystem::Cargo, "a\u{0}b"),
            (Ecosystem::Cargo, "a/b"),
            (Ecosystem::Cargo, "a%2fb"),
            (Ecosystem::Cargo, "a?b"),
            (Ecosystem::Cargo, "a#b"),
            (Ecosystem::Cargo, "a\\b"),
            (Ecosystem::Cargo, "1crate"),
            (Ecosystem::Cargo, "crate.name"),
            (Ecosystem::Npm, "-x"),
            (Ecosystem::Npm, "@scope"),
            (Ecosystem::Npm, "@scope/a/b"),
            (Ecosystem::Npm, "scope/name"),
            (Ecosystem::Npm, ".hidden"),
            (Ecosystem::Npm, "_hidden"),
            (Ecosystem::Npm, "@scope/-x"),
            (Ecosystem::Pypi, "-x"),
            (Ecosystem::Pypi, "pkg/../etc"),
            (Ecosystem::Pypi, ".pkg"),
            (Ecosystem::Pypi, "pkg-"),
            (Ecosystem::Pypi, "pkg name"),
        ];
        for (ecosystem, name) in hazards {
            assert!(
                matches!(
                    ReleasePackageId::new(ecosystem, name),
                    Err(ReleasePackageIdParseError::UnsafeName { .. } | ReleasePackageIdParseError::Malformed { .. })
                ),
                "{ecosystem:?} accepted an unsafe package name: {name:?}"
            );
        }
    }

    /// The real names each ecosystem publishes under must keep working.
    #[test]
    fn release_package_names_accept_each_ecosystem_grammar() {
        for (ecosystem, name) in [
            (Ecosystem::Cargo, "callisto-model"),
            (Ecosystem::Cargo, "_private_crate"),
            (Ecosystem::Cargo, "serde"),
            (Ecosystem::Npm, "callisto"),
            (Ecosystem::Npm, "@orin-dx/callisto"),
            (Ecosystem::Npm, "left.pad"),
            (Ecosystem::Npm, "JSONStream"),
            (Ecosystem::Npm, "Base64"),
            (Ecosystem::Npm, "@_scope/_x"),
            (Ecosystem::Npm, "@types/node"),
            (Ecosystem::Pypi, "typing-extensions"),
            (Ecosystem::Pypi, "zope.interface"),
            (Ecosystem::Pypi, "Flask"),
        ] {
            let id = ReleasePackageId::new(ecosystem, name)
                .unwrap_or_else(|error| panic!("{ecosystem:?}/{name} must stay valid: {error}"));
            assert_eq!(id.name(), name);
        }
    }
}
