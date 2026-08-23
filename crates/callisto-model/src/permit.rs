//! The capability token that gates every side-effecting operation in callisto.
//!
//! Before this existed, "respect `--dry-run`" was a convention: commands
//! were expected to check a bool before writing. Several forgot (`pre
//! enter`/`pre exit`, `init`) -- the same defect shape cargo's own
//! `--dry-run` tracker has. A convention every new write site must
//! remember will eventually be forgotten at one.
//!
//! [`ApplyPermit`] converts that convention into a type obligation. Write
//! primitives (`atomic_write`, manifest persistence, tag creation, registry
//! publishing, git staging) take `&ApplyPermit`; the only way to obtain one
//! outside tests is [`ApplyPermit::granted_unless_dry_run`], which returns
//! `None` for a dry run. A handler that forgets the check has nothing to
//! pass, and fails to compile.

/// Proof that the caller is authorized to perform real side effects.
///
/// Hold one to write to disk, create git refs, or publish to a registry.
/// The private unit field means no module -- inside this crate or outside
/// it -- can construct one via a struct literal; the constructors below
/// are the entire surface.
///
/// ```
/// use callisto_model::ApplyPermit;
///
/// // A dry run yields no permit, so no write primitive can be called.
/// assert!(ApplyPermit::granted_unless_dry_run(true).is_none());
///
/// // A real run yields one, which is then threaded into write primitives.
/// let permit = ApplyPermit::granted_unless_dry_run(false).expect("not a dry run");
/// # let _ = permit;
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ApplyPermit {
    _private: (),
}

impl ApplyPermit {
    /// The one sanctioned construction path: mints a permit unless a dry run
    /// was requested.
    ///
    /// `dry_run` should come straight from the parsed CLI flag, read exactly
    /// once near the top of a command handler. Recomputing or re-deriving it
    /// deeper in a call stack reintroduces the divergence this type exists to
    /// eliminate.
    #[must_use]
    pub fn granted_unless_dry_run(dry_run: bool) -> Option<Self> {
        if dry_run {
            None
        } else {
            Some(Self { _private: () })
        }
    }

    /// Test-only escape hatch for exercising write primitives directly,
    /// without routing through a command handler's flag plumbing.
    ///
    /// Named loudly on purpose: any occurrence outside a test module is
    /// immediately visible in review and to `rg force_for_tests`. Requires the
    /// `test-util` feature when used from another crate's tests.
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn force_for_tests() -> Self {
        Self { _private: () }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_run_yields_no_permit() {
        assert!(ApplyPermit::granted_unless_dry_run(true).is_none());
    }

    #[test]
    fn real_run_yields_a_permit() {
        assert!(ApplyPermit::granted_unless_dry_run(false).is_some());
    }
}
