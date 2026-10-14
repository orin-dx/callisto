//! Durable, forge-neutral release identity and digest primitives.
//!
//! This module deliberately owns only values and deterministic byte encodings.
//! Workspace discovery and release authorization remain in callisto-graph.

use crate::{Version, VersionGrammar};

mod artifact;
mod decision;
mod digest;
mod execution;
mod github;
mod intent;
mod operation;
mod package_id;
mod report;
mod snapshot;

#[cfg(test)]
mod tests;

pub use artifact::*;
pub use decision::*;
pub use digest::*;
pub use execution::*;
pub use github::*;
pub use intent::*;
pub use operation::*;
pub use package_id::*;
pub use report::*;
pub use snapshot::*;

/// Sort key for a `Version` field inside a hand-written `Ord` impl.
///
/// `(grammar, raw)` is exactly the subset of `Version`'s own derived
/// `PartialEq`/`Eq` fields needed to keep a containing type's `Ord`
/// consistent with its `Eq`: `parsed` is a pure function of `(grammar,
/// raw)` (see `Version::parse`), so it draws no further distinctions once
/// grammar and raw already agree. `render()` (== `raw`) alone is NOT
/// enough -- the same literal string parses under more than one grammar
/// (e.g. `"1.2.3"` is valid SemVer *and* PEP 440), so two `Version`s with
/// different `grammar` but identical `render()` output would compare
/// unequal via derived `Eq` yet `Ordering::Equal` via a `render()`-only
/// `Ord`, violating the invariant `BTreeSet`/`BTreeMap` require:
/// `a.cmp(b) == Equal` iff `a == b`.
fn version_ord_key(v: &Version) -> (VersionGrammar, &str) {
    (v.grammar(), v.render())
}

/// Rejects a wire schema version that doesn't match `expected`, with the
/// exact "unsupported {type_name} schema version" wording every schema-gated
/// `Deserialize` impl in this crate uses. Centralizing this comparison means
/// a schema bump that adds a nested versioned field (as `ReleaseIntentV1`
/// does for `decision`/`snapshot`) can't forget to gate it the way a
/// hand-copied check could.
pub(crate) fn check_schema_version<E: serde::de::Error>(found: u8, expected: u8, type_name: &str) -> Result<(), E> {
    if found != expected {
        return Err(E::custom(format!(
            "unsupported {type_name} schema version {found}; this build reads version {expected}"
        )));
    }
    Ok(())
}
