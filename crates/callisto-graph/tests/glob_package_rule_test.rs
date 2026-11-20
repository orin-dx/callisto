//! Tests for ecosystem-prefixed glob `[[package]]` matching and the
//! zero-match diagnostic.
//!
//! `"cargo/pkg-*"` matches only Cargo packages, so matching must consider the ecosystem prefix, not just the name.
//! A rule matching nothing at the walk-time call site must raise `DiagnosticCode::PackageRuleMatchedNothing`.

use std::fs;
use std::path::Path;

use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::DependencyResolver;
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

/// An ecosystem-prefixed glob `[[package]]` rule that matches zero real
/// packages in the workspace must surface a `PackageRuleMatchedNothing`
/// diagnostic, not fail silently.
///
/// The workspace has one Cargo-ecosystem package named "internal-foo". The
/// rule `"npm/internal-*"` requires an npm-ecosystem package, which does not
/// exist here, so it must match nothing and be surfaced.
#[test]
fn ecosystem_prefixed_glob_rule_matching_nothing_emits_diagnostic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    let crate_dir = root.join("internal-foo");
    fs::create_dir_all(&crate_dir).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"internal-foo\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        crate_dir.join("Cargo.toml"),
        "[package]\nname = \"internal-foo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"npm/internal-*\"\nrelease-trigger = \"auto\"\n",
    )
    .unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace should load without error");

    let matching: Vec<_> = ws
        .graph
        .diagnostics()
        .iter()
        .filter(|d| d.code == DiagnosticCode::PackageRuleMatchedNothing)
        .collect();

    assert_eq!(
        matching.len(),
        1,
        "expected exactly 1 PackageRuleMatchedNothing diagnostic, got {}; \
         all diagnostics: {:?}",
        matching.len(),
        ws.graph.diagnostics(),
    );

    assert!(
        matching[0].message.contains("npm/internal-*"),
        "diagnostic message must name the unmatched pattern; got: {:?}",
        matching[0].message,
    );
}

/// An ecosystem-prefixed glob `[[package]]` rule that DOES match a real
/// package (correct ecosystem, matching name) must apply its override and
/// must NOT emit a PackageRuleMatchedNothing diagnostic.
#[test]
fn ecosystem_prefixed_glob_rule_matching_real_package_applies_and_is_silent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    let crate_dir = root.join("internal-foo");
    fs::create_dir_all(&crate_dir).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"internal-foo\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        crate_dir.join("Cargo.toml"),
        "[package]\nname = \"internal-foo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"cargo/internal-*\"\nrelease-trigger = \"auto\"\n",
    )
    .unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace should load without error");

    let matching: Vec<_> = ws
        .graph
        .diagnostics()
        .iter()
        .filter(|d| d.code == DiagnosticCode::PackageRuleMatchedNothing)
        .collect();

    assert!(
        matching.is_empty(),
        "'cargo/internal-*' matches the real Cargo package 'internal-foo'; \
         no PackageRuleMatchedNothing diagnostic expected, got: {:?}",
        matching,
    );

    let pkg = ws
        .graph
        .packages()
        .find(|p| p.id.name() == "internal-foo")
        .expect("internal-foo package must exist");
    assert_eq!(
        pkg.release_trigger,
        callisto_model::ReleaseTrigger::Auto,
        "the cargo/internal-* [[package]] override must have applied"
    );
}
