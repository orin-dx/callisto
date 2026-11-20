use callisto_model::{Ecosystem, PackageId};
use globset::{Glob, GlobMatcher};

/// What a `[[package]]` rule's `match` names: one package, or every package whose name fits a glob.
#[derive(Clone, Debug)]
pub enum PackageMatch {
    /// `ecosystem/name`: the package's own name in that ecosystem.
    Exact(PackageId),
    Glob(PackagePattern),
}

impl PackageMatch {
    /// A `match` is a glob when it contains any of `*?[]{}`: no npm, Cargo or PyPI name can.
    pub(super) fn parse(raw: &str) -> Result<Self, String> {
        if raw.contains(['*', '?', '[', ']', '{', '}']) {
            PackagePattern::parse(raw).map(Self::Glob).map_err(|e| e.to_string())
        } else {
            PackageId::parse(raw).map(Self::Exact).map_err(|e| e.to_string())
        }
    }

    /// Whether the rule applies to a package with these native names, one per ecosystem it has a manifest in.
    pub(crate) fn applies(&self, native_names: &[(Ecosystem, &str)]) -> bool {
        match self {
            Self::Exact(id) => super::resolve::rule_applies(id, native_names),
            Self::Glob(pattern) => pattern.matches_native(native_names),
        }
    }
}

impl std::fmt::Display for PackageMatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exact(id) => id.fmt(f),
            Self::Glob(pattern) => pattern.fmt(f),
        }
    }
}

/// A glob over package names, the glob form of a `[[package]]` `match`.
///
/// One rule can target many packages at once (e.g. `"pkg-*"` matches `pkg-a`, `pkg-b`, etc.).
///
/// The ecosystem prefix is respected: `"cargo/pkg-*"` only matches Cargo packages. Config loading requires the
/// prefix; a bare pattern parses here but matches every ecosystem.
#[derive(Clone, Debug)]
pub struct PackagePattern {
    raw: String,
    /// The ecosystem parsed off the front of the pattern (e.g. `cargo` in
    /// `"cargo/pkg-*"`), or `None` for a bare pattern like `"pkg-*"`.
    ecosystem: Option<Ecosystem>,
    /// Compiled from the pattern with any `ecosystem/` prefix stripped.
    matcher: GlobMatcher,
}

impl PackagePattern {
    pub fn parse(s: &str) -> Result<Self, globset::Error> {
        let (ecosystem, rest) = s
            .split_once('/')
            .and_then(|(prefix, rest)| Ecosystem::from_prefix(prefix).map(|eco| (Some(eco), rest)))
            .unwrap_or((None, s));
        let glob = Glob::new(rest)?;
        Ok(Self {
            raw: s.to_string(),
            ecosystem,
            matcher: glob.compile_matcher(),
        })
    }

    /// Returns the ecosystem this pattern was scoped to via an
    /// `ecosystem/` prefix, or `None` for a bare pattern.
    pub fn ecosystem(&self) -> Option<Ecosystem> {
        self.ecosystem
    }

    /// Returns true when the given `PackageId` matches this pattern.
    ///
    /// A bare pattern (no `ecosystem/` prefix) matches any ecosystem, so
    /// `"pkg-*"` matches both `cargo/pkg-a` and `npm/pkg-b`. An
    /// ecosystem-prefixed pattern like `"cargo/pkg-*"` matches only packages
    /// in that ecosystem — `id.ecosystem()` must equal the parsed prefix
    /// before the glob remainder is compared against `id.name()`.
    pub fn matches(&self, id: &PackageId) -> bool {
        self.matches_in_ecosystems(id.name(), id.ecosystem().as_slice())
    }

    /// Like [`Self::matches`], but checks a name against an explicit set of
    /// candidate ecosystems rather than a single `PackageId`.
    ///
    /// This is needed at walk time: discovered package ids are
    /// [`PackageId::Bare`] (no ecosystem attached to the id itself), so the
    /// real ecosystem(s) of the package — sourced from its manifests — must
    /// be supplied explicitly for an ecosystem-prefixed pattern to ever be
    /// able to match.
    pub fn matches_in_ecosystems(&self, name: &str, ecosystems: &[Ecosystem]) -> bool {
        if let Some(want) = self.ecosystem
            && !ecosystems.contains(&want)
        {
            return false;
        }
        self.matcher.is_match(name)
    }

    /// Whether any of a package's native names, one per ecosystem it has a manifest in, matches this pattern: the
    /// ecosystem must be the pattern's when it names one, and the glob is compared with that ecosystem's own name.
    pub fn matches_native(&self, names: &[(Ecosystem, &str)]) -> bool {
        names.iter().any(|(ecosystem, name)| {
            self.ecosystem.is_none_or(|want| want == *ecosystem) && self.matcher.is_match(name)
        })
    }
}

impl std::fmt::Display for PackagePattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ecosystem-prefixed pattern like `"cargo/internal-*"` must match only
    /// packages in that ecosystem, not any package with a matching name
    /// regardless of ecosystem.
    #[test]
    fn ecosystem_prefixed_pattern_matches_only_that_ecosystem() {
        let pattern = PackagePattern::parse("cargo/internal-*").expect("valid glob");

        let cargo_pkg = PackageId::parse("cargo:internal-foo").expect("valid package id");
        let npm_pkg = PackageId::parse("npm:internal-foo").expect("valid package id");

        assert!(
            pattern.matches(&cargo_pkg),
            "'cargo/internal-*' must match the Cargo package 'internal-foo'"
        );
        assert!(
            !pattern.matches(&npm_pkg),
            "'cargo/internal-*' must NOT match the npm package 'internal-foo'"
        );
    }

    /// `eco/glob` must be accepted as an alias of `eco:glob` -- before the
    /// fix, `/` was never checked as a separator, so the whole string
    /// (`"cargo/foo-*"`) was compiled as one literal glob that could never
    /// match a bare package name like `foo-a`.
    #[test]
    fn slash_separated_pattern_matches_only_that_ecosystem() {
        let pattern = PackagePattern::parse("cargo/foo-*").expect("valid glob");
        let cargo_pkg = PackageId::parse("cargo:foo-a").expect("valid package id");
        let npm_pkg = PackageId::parse("npm:foo-a").expect("valid package id");

        assert!(
            pattern.matches(&cargo_pkg),
            "'cargo/foo-*' must match the Cargo package 'foo-a'"
        );
        assert!(
            !pattern.matches(&npm_pkg),
            "'cargo/foo-*' must not match the npm package 'foo-a'"
        );
    }
}
