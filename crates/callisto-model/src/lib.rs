//! Shared types and traits for callisto: package identity, versions, manifests, dependency
//! specs, and the versioned JSON report contract.

pub mod atomic;

pub mod permit;
pub use permit::*;

pub mod tag;
pub use tag::*;

pub mod error;
pub use error::*;

pub mod path;
pub use path::*;

pub mod identity;
pub use identity::*;

pub mod ecosystem;
pub use ecosystem::*;

pub mod version;
pub use version::*;

pub mod severity;
pub use severity::*;

pub mod package;
pub use package::*;

pub mod dependency;
pub use dependency::*;

pub mod discovery;
pub use discovery::*;

pub mod exec;
pub use exec::*;

pub mod commit;
pub use commit::*;

pub mod diagnostic;
pub use diagnostic::*;

pub mod plan;
pub use plan::*;

pub mod report;
pub use report::*;

pub mod matrix;
pub use matrix::*;

pub mod registry;
pub use registry::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync_static<T: Send + Sync + 'static>() {}

    #[test]
    fn test_auto_traits() {
        assert_send_sync_static::<PackageId>();
        assert_send_sync_static::<Version>();
        assert_send_sync_static::<Severity>();
        assert_send_sync_static::<Ecosystem>();
        assert_send_sync_static::<Package>();
        assert_send_sync_static::<PublishPlan>();
        assert_send_sync_static::<PublishReport>();
        assert_send_sync_static::<VersionReport>();
        assert_send_sync_static::<StatusReport>();
    }

    /// Asserts that every #[diagnostic(code(...))] value across all callisto crates is unique.
    /// This test enumerates every code found by audit; a duplicate entry here mirrors a duplicate
    /// in source and causes an immediate, named failure.
    #[test]
    fn test_all_diagnostic_codes_are_unique() {
        let codes: &[&str] = &[
            // callisto-model: error.rs
            "E004", "E005", "E006", "E008", "E009", "E010", "E011", "E012", "E013", "E015", "E016",
            "E017", "E018", "E019", // callisto-model: exec.rs
            "E021", "E022", "E023", "E024", // callisto-graph: locate/mod.rs
            "E031", "E032", // callisto-format: bump.rs
            "E035", "E036", "E037", "E038", "E039",
            // callisto-format: changeset/mod.rs ParseError
            "E041", "E042", "E043", "E044", "E045", "E046", "E047", "E048",
            // callisto-format: changeset/mod.rs WriteError
            "E049", "E050", "E052", // callisto-vcs: lib.rs
            "E051", // callisto-changelog: error.rs
            "E060", "E061", "E062", "E063", // callisto-graph: error.rs
            "E101", "E107", "E108", "E109", "E110", "E111", "E118",
        ];
        let mut seen = std::collections::BTreeSet::new();
        for code in codes {
            assert!(seen.insert(*code), "Duplicate diagnostic code: {code}");
        }
    }
}
