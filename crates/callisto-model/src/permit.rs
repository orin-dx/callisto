//! The capability token that gates every side-effecting operation in callisto.
//!
//! Before this existed, "respect `--dry-run`" was a convention: each command
//! handler was expected to remember to consult a bool before writing. Four
//! separate commands forgot -- `version` and `add` (fixed earlier), then
//! `pre enter`/`pre exit` and `init`, neither of which read the flag at all.
//! The bug class is not specific to this codebase; cargo's own `--dry-run`
//! tracker carries the same shape of defect. A convention that must be
//! remembered at every new write site will eventually be forgotten at one.
//!
//! [`ApplyPermit`] converts that convention into a type obligation. Write
//! primitives (`atomic_write`, manifest persistence, tag creation, registry
//! publishing, git staging) take `&ApplyPermit`, and the only way to obtain
//! one outside of tests is [`ApplyPermit::granted_unless_dry_run`], which
//! returns `None` for a dry run. A command handler that forgets the check now
//! has nothing to pass, and fails to compile.

/// Proof that the caller is authorized to perform real side effects.
///
/// Hold one of these and you may write to disk, create git refs, or publish to
/// a registry. The private unit field means no other module -- inside this
/// crate or outside it -- can construct one via a struct literal; the
/// constructors below are the entire surface.
///
/// # Examples
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
