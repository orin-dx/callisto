//! Regression test for F16: redundant identity resolution.
//!
//! `IdentityResolver::resolve` used to do a fresh `std::fs::read_to_string` +
//! full manifest parse on every single call with zero memoization, so a
//! widely-depended-on package (e.g. a shared utils crate with many internal
//! dependents) had its manifest read and parsed once per caller instead of
//! once total. `IdentityResolver::resolve` now memoizes by `(path,
//! ecosystem)`, so repeated resolution of the same identity is served from
//! the memo after the first call.

use callisto_graph::identity::{identity_read_count, reset_identity_read_count, IdentityResolver};
use callisto_model::Ecosystem;
use serial_test::serial;

#[test]
#[serial]
fn resolve_reads_a_given_manifest_at_most_once_regardless_of_call_count() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"widely-depended-on\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let resolver = IdentityResolver::new(dir.path()).unwrap();

    // This counter is process-global (see callisto_graph::identity::identity_read_count).
    // This test must not run concurrently with other tests in this binary
    // that also resolve real manifests, so it is the sole
    // resolve()-exercising test in this file.
    reset_identity_read_count();

    // Simulate 30 independent callers (e.g. 30 dependency edges all naming
    // the same shared package) each resolving the identical identity.
    for _ in 0..30 {
        let id = resolver.resolve(std::path::Path::new("."), Ecosystem::Cargo).unwrap();
        assert_eq!(id.name(), "widely-depended-on");
    }

    assert_eq!(
        identity_read_count(),
        1,
        "30 resolve() calls for the identical (path, ecosystem) pair must read/parse \
         the manifest exactly once, not once per call"
    );
}
