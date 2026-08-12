use callisto_model::{Ecosystem, PackageId};
use globset::{Glob, GlobMatcher};

/// A glob pattern for matching package names in `[[package-set]]` blocks.
///
/// Unlike `[[package]]` which uses an exact `PackageId` match,
/// `[[package-set]]` allows a single rule to target many packages at once
/// via a glob (e.g. `"pkg-*"` matches `pkg-a`, `pkg-b`, etc.).
///
/// Ecosystem prefixes in the pattern are respected: `"cargo:pkg-*"` only
/// matches Cargo packages; a bare `"pkg-*"` matches packages in any ecosystem
/// that have a matching name.
#[derive(Clone, Debug)]
pub struct PackagePattern {
    raw: String,
    /// The ecosystem parsed off the front of the pattern (e.g. `cargo` in
    /// `"cargo:pkg-*"`), or `None` for a bare pattern like `"pkg-*"`.
    ecosystem: Option<Ecosystem>,
    /// Compiled from the pattern with any `ecosystem:` prefix stripped.
    matcher: GlobMatcher,
}

impl PackagePattern {
    pub fn parse(s: &str) -> Result<Self, globset::Error> {
        let (ecosystem, rest) = match s.split_once(':') {
            Some((prefix, rest)) => match Ecosystem::from_prefix(prefix) {
                Some(eco) => (Some(eco), rest),
                None => (None, s),
            },
            None => (None, s),
        };
        let glob = Glob::new(rest)?;
        Ok(Self {
            raw: s.to_string(),
            ecosystem,
            matcher: glob.compile_matcher(),
        })
    }

    /// Returns the ecosystem this pattern was scoped to via an
    /// `ecosystem:` prefix, or `None` for a bare pattern.
    pub fn ecosystem(&self) -> Option<Ecosystem> {
        self.ecosystem
    }

    /// Returns true when the given `PackageId` matches this pattern.
    ///
    /// A bare pattern (no `ecosystem:` prefix) matches any ecosystem, so
    /// `"pkg-*"` matches both `cargo:pkg-a` and `npm:pkg-b`. An
    /// ecosystem-prefixed pattern like `"cargo:pkg-*"` matches only packages
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
        if let Some(want) = self.ecosystem {
            if !ecosystems.contains(&want) {
                return false;
            }
        }
        self.matcher.is_match(name)
    }

    pub fn as_str(&self) -> &str {
        &self.raw
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

    /// An ecosystem-prefixed pattern like `"cargo:internal-*"` must match only
    /// packages in that ecosystem, not any package with a matching name
    /// regardless of ecosystem. Before the fix, `matches()` glob-matched
    /// against `id.name()` alone (which never contains the `cargo:` prefix
    /// baked into the compiled glob), so an ecosystem-prefixed pattern matched
    /// zero packages of any ecosystem.
    #[test]
    fn ecosystem_prefixed_pattern_matches_only_that_ecosystem() {
        let pattern = PackagePattern::parse("cargo:internal-*").expect("valid glob");

        let cargo_pkg = PackageId::parse("cargo:internal-foo").expect("valid package id");
        let npm_pkg = PackageId::parse("npm:internal-foo").expect("valid package id");

        assert!(
            pattern.matches(&cargo_pkg),
            "'cargo:internal-*' must match the Cargo package 'internal-foo'"
        );
        assert!(
            !pattern.matches(&npm_pkg),
            "'cargo:internal-*' must NOT match the npm package 'internal-foo'"
        );
    }
}
