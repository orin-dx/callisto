//! Each error code names one condition: a user's mistake never reports the internal-defect code, and
//! `callisto.toml` syntax errors, unknown keys and misfit values report different codes.
#[path = "common/release_harness.rs"]
mod release_harness;

use release_harness::*;

fn stderr_of(root: &std::path::Path, args: &[&str]) -> String {
    let out = callisto(root, args);
    assert!(!out.status.success(), "expected failure for {args:?}");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn assert_only_code(stderr: &str, expected: &str) {
    for code in ["E111", "E325", "E326", "E327", "E328", "E171"] {
        assert_eq!(
            stderr.contains(code),
            code == expected,
            "{expected} only, got:\n{stderr}"
        );
    }
}

#[test]
fn pending_from_that_is_not_a_sha_is_a_user_error_not_an_internal_invariant() {
    let (dir, _) = release_commit_fixture();
    assert_only_code(&stderr_of(dir.path(), &["release", "pending", "--from", "abc"]), "E327");
}

#[test]
fn pending_from_a_commit_that_is_not_a_release_commit_is_a_user_error() {
    let (dir, _) = release_commit_fixture();
    let missing = "a".repeat(40);
    assert_only_code(
        &stderr_of(dir.path(), &["release", "pending", "--from", &missing]),
        "E328",
    );
}

fn config_error(toml: &str) -> String {
    let (dir, _) = release_commit_fixture();
    std::fs::write(dir.path().join("callisto.toml"), toml).unwrap();
    stderr_of(dir.path(), &["status"])
}

#[test]
fn config_unknown_key_reports_e325() {
    assert_only_code(&config_error("bogus = 1\n"), "E325");
    assert_only_code(&config_error("[changesets]\nbogus = 1\n"), "E325");
}

#[test]
fn config_syntax_error_reports_e111() {
    assert_only_code(&config_error("[changesets\n"), "E111");
}

#[test]
fn config_value_of_the_wrong_type_reports_e326() {
    assert_only_code(&config_error("[changesets]\ndir = 5\n"), "E326");
}

#[test]
fn every_new_code_is_documented() {
    let doc = include_str!("../../../docs/errors.md");
    for code in ["E111", "E325", "E326", "E327", "E328"] {
        assert!(
            doc.contains(&format!("| {code} |")),
            "{code} missing from docs/errors.md"
        );
    }
}
