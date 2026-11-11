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

/// The error code is `expected` and neither the retired catch-all nor the internal-defect code.
fn assert_code(stderr: &str, expected: &str) {
    assert!(stderr.contains(expected), "expected {expected}, got:\n{stderr}");
    for wrong in ["E254", "E171"] {
        assert!(!stderr.contains(wrong), "{expected} expected, not {wrong}:\n{stderr}");
    }
}

#[test]
fn a_bad_publish_to_value_reports_e330() {
    let stderr = config_error("[[package]]\nmatch = \"cargo/core-crate\"\npublish-to = [\"bogus\"]\n");
    assert_code(&stderr, "E330");
    assert!(stderr.contains("bogus"), "{stderr}");
    assert_code(
        &config_error("[[package-set]]\nmatch = \"cargo/*\"\npublish-to = [\"bogus\"]\n"),
        "E330",
    );
}

#[test]
fn a_bad_release_trigger_reports_e331() {
    assert_code(
        &config_error("[[package]]\nmatch = \"cargo/core-crate\"\nrelease-trigger = \"sometimes\"\n"),
        "E331",
    );
    assert_code(
        &config_error("[[package-set]]\nmatch = \"cargo/*\"\nrelease-trigger = \"sometimes\"\n"),
        "E331",
    );
}

#[test]
fn a_bad_cascade_mode_reports_e332() {
    assert_code(&config_error("[cascade]\nmode = \"sometimes\"\n"), "E332");
}

#[test]
fn a_bad_changelog_layout_reports_e333() {
    assert_code(&config_error("[changelog]\nlayout = \"tree\"\n"), "E333");
}

#[test]
fn an_unknown_registry_kind_reports_e334() {
    assert_code(&config_error("[registries.corp]\nkind = \"maven\"\n"), "E334");
}

#[test]
fn a_publish_url_on_a_registry_that_is_not_pypi_reports_e335() {
    assert_code(
        &config_error("[registries.corp]\nkind = \"npm\"\npublish-url = \"https://corp.example.com/up\"\n"),
        "E335",
    );
}

#[test]
fn a_url_on_a_cargo_registry_reports_e336() {
    assert_code(
        &config_error("[registries.cratesIo]\nurl = \"https://corp.example.com/index\"\n"),
        "E336",
    );
}

#[test]
fn an_unusable_registry_timeout_reports_e337() {
    assert_code(&config_error("[registries.npm]\nconfirm-timeout = \"soon\"\n"), "E337");
    assert_code(&config_error("[registries.npm]\npublish-timeout = \"0s\"\n"), "E337");
}

#[test]
fn an_unparsable_package_match_reports_e338() {
    assert_code(&config_error("[[package]]\nmatch = \"cargo/a..b\"\n"), "E338");
    assert_code(&config_error("[[package-set]]\nmatch = \"cargo/[\"\n"), "E338");
}

#[test]
fn a_registry_of_the_wrong_kind_for_the_publish_target_reports_e339() {
    let (dir, _) = release_commit_fixture();
    let config = dir.path().join("callisto.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str("\n[registries.cratesIo]\nkind = \"npm\"\n");
    std::fs::write(config, text).unwrap();
    assert_code(&stderr_of(dir.path(), &["release", "--dry-run"]), "E339");
}

#[test]
fn every_config_value_code_is_documented() {
    let doc = include_str!("../../../docs/errors.md");
    for code in [
        "E330", "E331", "E332", "E333", "E334", "E335", "E336", "E337", "E338", "E339",
    ] {
        assert!(
            doc.contains(&format!("| {code} |")),
            "{code} missing from docs/errors.md"
        );
    }
}
