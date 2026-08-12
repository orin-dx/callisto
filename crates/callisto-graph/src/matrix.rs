#![allow(dead_code)]
// Removed in T12 once crates/callisto-graph/src/commands/matrix.rs (the
// public entry point) becomes a non-test caller of this module's
// pub(crate) functions; until then `cargo clippy --all-targets -- -D
// warnings` flags every function in this file as dead_code.

//! Manifest reading and MatrixReport assembly for `callisto matrix`.
//! Private helpers only -- the public entry point is
//! `crate::commands::matrix::matrix`.

/// Per-triple CI scheduling attributes for the 18 triples
/// `crate::napi::triple_to_role` recognises today. This table is NEW code:
/// `triple_to_role`'s `ManifestRole::Platform` carries only platform/arch/abi
/// and has no concept of hostRunner/useCross, so this cannot be derived from
/// it. Returns `None` for any triple `triple_to_role` does not recognise --
/// callers must treat that as the AC-011 diagnostic path, never a silent
/// default.
pub(crate) fn triple_host_runner_use_cross(triple: &str) -> Option<(&'static str, bool)> {
    Some(match triple {
        "aarch64-apple-darwin" => ("macos-latest", false),
        "x86_64-apple-darwin" => ("macos-13", false),
        "x86_64-unknown-linux-gnu" => ("ubuntu-latest", false),
        "x86_64-unknown-linux-musl" => ("ubuntu-latest", true),
        "aarch64-unknown-linux-gnu" => ("ubuntu-latest", true),
        "aarch64-unknown-linux-musl" => ("ubuntu-latest", true),
        "x86_64-pc-windows-msvc" => ("windows-latest", false),
        "i686-pc-windows-msvc" => ("windows-latest", false),
        "aarch64-pc-windows-msvc" => ("windows-latest", false),
        "armv7-unknown-linux-gnueabihf" => ("ubuntu-latest", true),
        "x86_64-unknown-freebsd" => ("ubuntu-latest", true),
        "aarch64-linux-android" => ("ubuntu-latest", true),
        "armv7-linux-androideabi" => ("ubuntu-latest", true),
        "riscv64gc-unknown-linux-gnu" => ("ubuntu-latest", true),
        "powerpc64le-unknown-linux-gnu" => ("ubuntu-latest", true),
        "s390x-unknown-linux-gnu" => ("ubuntu-latest", true),
        "wasm32-wasip1" => ("ubuntu-latest", false),
        "wasm32-unknown-unknown" => ("ubuntu-latest", false),
        _ => return None,
    })
}

/// AC-013: artifactName is always the literal "native-" concatenated with
/// the triple string, for every recognised triple.
pub(crate) fn artifact_name_for_triple(triple: &str) -> String {
    format!("native-{triple}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AC-012: table-driven assertion of all 18 (hostRunner, useCross) pairs.
    #[test]
    fn triple_host_runner_use_cross_matches_all_18_triples() {
        let expected: &[(&str, &str, bool)] = &[
            ("aarch64-apple-darwin", "macos-latest", false),
            ("x86_64-apple-darwin", "macos-13", false),
            ("x86_64-unknown-linux-gnu", "ubuntu-latest", false),
            ("x86_64-unknown-linux-musl", "ubuntu-latest", true),
            ("aarch64-unknown-linux-gnu", "ubuntu-latest", true),
            ("aarch64-unknown-linux-musl", "ubuntu-latest", true),
            ("x86_64-pc-windows-msvc", "windows-latest", false),
            ("i686-pc-windows-msvc", "windows-latest", false),
            ("aarch64-pc-windows-msvc", "windows-latest", false),
            ("armv7-unknown-linux-gnueabihf", "ubuntu-latest", true),
            ("x86_64-unknown-freebsd", "ubuntu-latest", true),
            ("aarch64-linux-android", "ubuntu-latest", true),
            ("armv7-linux-androideabi", "ubuntu-latest", true),
            ("riscv64gc-unknown-linux-gnu", "ubuntu-latest", true),
            ("powerpc64le-unknown-linux-gnu", "ubuntu-latest", true),
            ("s390x-unknown-linux-gnu", "ubuntu-latest", true),
            ("wasm32-wasip1", "ubuntu-latest", false),
            ("wasm32-unknown-unknown", "ubuntu-latest", false),
        ];
        assert_eq!(
            expected.len(),
            18,
            "sanity check: table must cover exactly 18 triples"
        );
        for &(triple, host_runner, use_cross) in expected {
            let (got_runner, got_cross) =
                triple_host_runner_use_cross(triple).unwrap_or_else(|| {
                    panic!("triple_host_runner_use_cross returned None for known triple `{triple}`")
                });
            assert_eq!(
                got_runner, host_runner,
                "hostRunner mismatch for `{triple}`"
            );
            assert_eq!(got_cross, use_cross, "useCross mismatch for `{triple}`");
        }
    }

    /// AC-013: artifactName is always "native-" + triple.
    #[test]
    fn artifact_name_is_native_prefixed_triple() {
        assert_eq!(
            artifact_name_for_triple("aarch64-apple-darwin"),
            "native-aarch64-apple-darwin"
        );
        assert_eq!(
            artifact_name_for_triple("x86_64-unknown-linux-musl"),
            "native-x86_64-unknown-linux-musl"
        );
    }

    /// An unrecognised triple must return None, not panic or fall back to a
    /// default -- callers use this to drive the AC-011 diagnostic path.
    #[test]
    fn triple_host_runner_use_cross_unknown_triple_returns_none() {
        assert!(triple_host_runner_use_cross("sparc64-unknown-linux-gnu").is_none());
    }
}
