//! Tests for the cross-ecosystem [[package]] rule warning diagnostic.
//!
//! Trigger: a SINGLE directory containing both Cargo.toml and package.json
//! (the napi case). IgnoreWalkLocator discovers both manifests for the same
//! directory and groups them into ONE packages-map entry whose `manifests` Vec
//! contains two canonical ManifestDecls (CargoToml + PackageJson). A bare
//! [[package]] rule that matches that entry then spans two distinct ecosystems,
//! and one BareRuleMatchesMultipleEcosystems diagnostic must be pushed.
//!
//! IMPORTANT: Do NOT write a fixture with two SEPARATE directories containing
//! packages of the same name — that triggers GraphError::DuplicatePackage and
//! Workspace::load returns Err, never reaching the diagnostic pass.

use std::fs;
use std::path::Path;

use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::Workspace;
use callisto_model::{CommandError, CommandOutput, CommandRunner, DiagnosticCode};

struct NoopRunner;

impl CommandRunner for NoopRunner {
    fn run(&self, _program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
        Ok(CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

/// A PREFIXED [[package]] rule must never trigger
/// BareRuleMatchesMultipleEcosystems even when the matched package has
/// canonical manifests in multiple ecosystems.
#[test]
fn prefixed_rule_never_triggers_cross_ecosystem_diagnostic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    // Napi-style package: one directory, both Cargo.toml and package.json.
    let pkg_dir = root.join("baz");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"baz\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(pkg_dir.join("package.json"), r#"{"name":"baz","version":"0.1.0"}"#).unwrap();
    // PREFIXED rule — pattern.ecosystem() returns Some, so the diagnostic pass
    // skips it unconditionally.
    fs::write(root.join("callisto.toml"), "[[package]]\nmatch = \"cargo/baz\"\n").unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace should load");

    let count = ws
        .graph
        .diagnostics()
        .iter()
        .filter(|d| d.code == DiagnosticCode::BareRuleMatchesMultipleEcosystems)
        .count();

    assert_eq!(
        count,
        0,
        "prefixed rules must not trigger BareRuleMatchesMultipleEcosystems; \
         all diagnostics: {:?}",
        ws.graph.diagnostics(),
    );
}

/// A [[package-set]] rule must never trigger the cross-ecosystem diagnostic
/// regardless of what it matches. The diagnostic pass iterates cfg.packages only,
/// never cfg.package_sets.
#[test]
fn package_set_rule_never_triggers_cross_ecosystem_diagnostic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    // Napi-style package: one directory, both Cargo.toml and package.json.
    let pkg_dir = root.join("qux");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"qux\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(pkg_dir.join("package.json"), r#"{"name":"qux","version":"0.1.0"}"#).unwrap();
    // [[package-set]] rule (not [[package]]) — must never trigger.
    // cfg.packages is empty; cfg.package_sets has one entry.
    fs::write(root.join("callisto.toml"), "[[package-set]]\nmatch = \"cargo/*\"\n").unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace should load");

    let count = ws
        .graph
        .diagnostics()
        .iter()
        .filter(|d| d.code == DiagnosticCode::BareRuleMatchesMultipleEcosystems)
        .count();

    assert_eq!(
        count,
        0,
        "[[package-set]] rules must not trigger BareRuleMatchesMultipleEcosystems; \
         all diagnostics: {:?}",
        ws.graph.diagnostics(),
    );
}

/// When cfg.packages is empty (no [[package]] rules in callisto.toml),
/// no BareRuleMatchesMultipleEcosystems diagnostic is present. The diagnostic
/// pass iterates cfg.packages, which is an empty Vec, so the loop body never
/// executes.
#[test]
fn empty_packages_config_emits_no_cross_ecosystem_diagnostic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    // Napi-style package (would trigger if there were a bare rule).
    let pkg_dir = root.join("zap");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"zap\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(pkg_dir.join("package.json"), r#"{"name":"zap","version":"0.1.0"}"#).unwrap();
    // callisto.toml with NO [[package]] rules — cfg.packages is empty.
    fs::write(root.join("callisto.toml"), "").unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace should load");

    let count = ws
        .graph
        .diagnostics()
        .iter()
        .filter(|d| d.code == DiagnosticCode::BareRuleMatchesMultipleEcosystems)
        .count();

    assert_eq!(
        count,
        0,
        "no [[package]] rules means no cross-ecosystem diagnostic; \
         all diagnostics: {:?}",
        ws.graph.diagnostics(),
    );
}

/// Cfg.packages is non-empty but the packages map has zero entries
/// (no packages discovered). The diagnostic loop body never fires because
/// packages.iter() yields nothing and the ecosystem set stays empty.
#[test]
fn non_empty_packages_config_with_empty_workspace_emits_no_diagnostic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    // No manifest files at all — packages map will have 0 entries.
    fs::write(
        root.join("callisto.toml"),
        r#"
[[package]]
match = "cargo/ghost"
"#,
    )
    .unwrap();
    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace should load with zero packages");
    let count = ws
        .graph
        .diagnostics()
        .iter()
        .filter(|d| d.code == DiagnosticCode::BareRuleMatchesMultipleEcosystems)
        .count();
    assert_eq!(
        count,
        0,
        "empty packages map means empty ecosystem set for every rule; \
         all diagnostics: {:?}",
        ws.graph.diagnostics(),
    );
}
