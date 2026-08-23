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

    /// Every source file across the workspace declaring at least one
    /// `#[diagnostic(code(E..))]`, embedded at compile time via
    /// `include_str!` (paths relative to this file). This list is the
    /// actual audit surface -- a file added here needs no further wiring,
    /// since the scan below extracts every code occurrence directly from
    /// the text.
    ///
    /// A prior version hand-maintained a `&[&str]` list of expected codes
    /// instead -- missed two entire files and a third of
    /// `callisto-graph/src/error.rs`'s codes, undetected until a duplicate
    /// (`E117` on two unrelated variants) surfaced by unrelated code
    /// review. Scanning real source text at compile time removes that
    /// maintenance step: a new file just needs adding here, and every
    /// code it ever gains is covered from then on.
    const DIAGNOSTIC_CODE_SOURCE_FILES: &[(&str, &str)] = &[
        ("callisto-model/src/error.rs", include_str!("error.rs")),
        ("callisto-model/src/exec.rs", include_str!("exec.rs")),
        ("callisto-model/src/commit.rs", include_str!("commit.rs")),
        ("callisto-model/src/version.rs", include_str!("version.rs")),
        (
            "callisto-graph/src/locate/mod.rs",
            include_str!("../../callisto-graph/src/locate/mod.rs"),
        ),
        (
            "callisto-graph/src/error.rs",
            include_str!("../../callisto-graph/src/error.rs"),
        ),
        (
            "callisto-format/src/bump.rs",
            include_str!("../../callisto-format/src/bump.rs"),
        ),
        (
            "callisto-format/src/changeset/mod.rs",
            include_str!("../../callisto-format/src/changeset/mod.rs"),
        ),
        ("callisto-vcs/src/lib.rs", include_str!("../../callisto-vcs/src/lib.rs")),
        (
            "callisto-changelog/src/error.rs",
            include_str!("../../callisto-changelog/src/error.rs"),
        ),
    ];

    /// Extracts every `code(E<digits>)` occurrence from `text`, in order of appearance. Deliberately
    /// a hand-rolled scan rather than a `regex` dependency — the pattern is fixed-shape and simple
    /// enough that adding a whole crate dependency for it isn't warranted.
    fn extract_diagnostic_codes(text: &str) -> Vec<String> {
        let mut codes = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find("code(E") {
            let after_marker = &rest[start + "code(".len()..];
            let digit_end = after_marker
                .find(|c: char| !c.is_ascii_digit() && c != 'E')
                .unwrap_or(after_marker.len());
            let candidate = &after_marker[..digit_end];
            // `code(` is also used for things like `code(callisto::foo)` in test-double
            // diagnostics elsewhere in the workspace (out of scope for this scan, since those
            // files aren't in `DIAGNOSTIC_CODE_SOURCE_FILES`) -- guard here anyway so a stray
            // non-numeric match can't silently produce a bogus "code".
            if candidate.len() > 1 && candidate[1..].chars().all(|c| c.is_ascii_digit()) {
                codes.push(candidate.to_string());
            }
            rest = &after_marker[digit_end..];
        }
        codes
    }

    /// Asserts that every `#[diagnostic(code(...))]` value across every file in
    /// [`DIAGNOSTIC_CODE_SOURCE_FILES`] is unique workspace-wide. Duplicate diagnostic codes are
    /// a real user-facing bug (E-codes are meant to be a stable, searchable identifier for one
    /// specific error condition) — see this test's doc comment on `DIAGNOSTIC_CODE_SOURCE_FILES`
    /// for the collision this replaced a broken, silently-incomplete version of the check.
    #[test]
    fn test_all_diagnostic_codes_are_unique() {
        let mut seen: std::collections::BTreeMap<String, &str> = std::collections::BTreeMap::new();
        for (file, text) in DIAGNOSTIC_CODE_SOURCE_FILES {
            for code in extract_diagnostic_codes(text) {
                if let Some(first_file) = seen.insert(code.clone(), file) {
                    panic!("Duplicate diagnostic code {code}: declared in both {first_file} and {file}");
                }
            }
        }
        assert!(
            seen.len() > 50,
            "expected at least 50 distinct diagnostic codes across the workspace (81 at the \
             time this test was written), got {} -- extract_diagnostic_codes likely stopped \
             matching real source (check DIAGNOSTIC_CODE_SOURCE_FILES's include_str! paths \
             still resolve)",
            seen.len()
        );
    }

    #[test]
    fn extract_diagnostic_codes_finds_every_code_in_a_small_fixture() {
        let text = r#"
            #[diagnostic(code(E001))]
            struct Foo;
            #[diagnostic(
                code(E042),
                help("do something")
            )]
            struct Bar;
        "#;
        assert_eq!(extract_diagnostic_codes(text), vec!["E001", "E042"]);
    }

    #[test]
    fn extract_diagnostic_codes_detects_a_duplicate_within_one_string() {
        let text = "code(E001) ... code(E001)";
        let codes = extract_diagnostic_codes(text);
        let mut seen = std::collections::BTreeSet::new();
        assert!(!codes.iter().all(|c| seen.insert(c.clone())));
    }
}
