use callisto_model::PackageId;
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
    matcher: GlobMatcher,
}

impl PackagePattern {
    pub fn parse(s: &str) -> Result<Self, globset::Error> {
        let glob = Glob::new(s)?;
        Ok(Self {
            raw: s.to_string(),
            matcher: glob.compile_matcher(),
        })
    }

    /// Returns true when the given `PackageId`'s name matches the glob pattern.
    ///
    /// Matching is done against the package name only (no ecosystem prefix),
    /// so `"pkg-*"` matches both `cargo:pkg-a` and `npm:pkg-b`.
    pub fn matches(&self, id: &PackageId) -> bool {
        self.matcher.is_match(id.name())
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
