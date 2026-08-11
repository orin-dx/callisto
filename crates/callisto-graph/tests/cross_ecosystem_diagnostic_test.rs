//! Tests for the cross-ecosystem [[package]] rule warning diagnostic (SPEC-002 AC-5).
//!
//! AC-5 trigger: a SINGLE directory containing both Cargo.toml and package.json
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
use callisto_model::{
    CommandError, CommandOutput, CommandRunner, DiagnosticCode, DiagnosticSeverity,
};

struct NoopRunner;

impl CommandRunner for NoopRunner {
    fn run(
        &self,
        _program: &str,
        _args: &[&str],
        _cwd: &Path,
    ) -> Result<CommandOutput, CommandError> {
        Ok(CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

/// AC-5 + AC-8b: A bare [[package]] rule matching a napi-style package (one
/// directory containing both Cargo.toml and package.json with the same name)
/// must emit exactly one BareRuleMatchesMultipleEcosystems Warning diagnostic.
/// The diagnostic must have escalated_by = None and governed_by = None (AC-8b).
/// The message must contain the rule name and both ecosystem prefixes.
#[test]
fn bare_rule_matching_napi_package_emits_one_cross_ecosystem_diagnostic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    // Napi-style fixture: ONE directory with BOTH Cargo.toml and package.json.
    // Both declare the same name "my-pkg". The IgnoreWalkLocator will group them
    // into a single by_path entry, producing one Package with two canonical
    // ManifestDecls (CargoToml + PackageJson). No workspace Cargo.toml is
    // needed — IgnoreWalkLocator walks the filesystem, not workspace members.
    let pkg_dir = root.join("my-pkg");
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"my-pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    // Plain package.json without `os`/`cpu` arrays so detect_npm_role returns
    // ManifestRole::Canonical (not Platform), giving two canonical ManifestDecls.
    fs::write(
        pkg_dir.join("package.json"),
        r#"{"name":"my-pkg","version":"0.1.0"}"#,
    )
    .unwrap();

    // Bare [[package]] rule — matches the single packages-map entry whose
    // canonical manifests span {Cargo, Npm}.
    fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"my-pkg\"\n",
    )
    .unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner)
        .expect("workspace should load without error");

    let matching: Vec<_> = ws
        .graph
        .diagnostics()
        .iter()
        .filter(|d| d.code == DiagnosticCode::BareRuleMatchesMultipleEcosystems)
        .collect();

    assert_eq!(
        matching.len(),
        1,
        "expected exactly 1 BareRuleMatchesMultipleEcosystems diagnostic, got {}; \
         all diagnostics: {:?}",
        matching.len(),
        ws.graph.diagnostics(),
    );

    let diag = matching[0];

    assert_eq!(
        diag.severity,
        DiagnosticSeverity::Warning,
        "diagnostic severity must be Warning, got {:?}",
        diag.severity,
    );
    assert!(
        diag.message.contains("my-pkg"),
        "diagnostic message must contain the rule name 'my-pkg'; got: {:?}",
        diag.message,
    );
    assert!(
        diag.message.contains("cargo"),
        "diagnostic message must contain ecosystem prefix 'cargo'; got: {:?}",
        diag.message,
    );
    assert!(
        diag.message.contains("npm"),
        "diagnostic message must contain ecosystem prefix 'npm'; got: {:?}",
        diag.message,
    );
    // AC-8b: advisory warning — no escalation, no governing key.
    assert!(
        diag.package.is_none(),
        "package must be None (AC-5); got: {:?}",
        diag.package,
    );
    assert!(
        diag.path.is_none(),
        "path must be None (AC-5); got: {:?}",
        diag.path,
    );
    assert!(
        diag.escalated_by.is_none(),
        "escalated_by must be None (AC-8b); got: {:?}",
        diag.escalated_by,
    );
    assert!(
        diag.governed_by.is_none(),
        "governed_by must be None (AC-8b); got: {:?}",
        diag.governed_by,
    );
}
