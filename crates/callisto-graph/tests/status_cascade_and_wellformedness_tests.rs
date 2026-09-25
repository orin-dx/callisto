//! SPEC-DX-STATUS-ADD AC-01 (pending severity is `plan_version`'s own
//! cascade/fixed-group computation, not a per-package changeset scan) and
//! AC-03 (`status` covers every well-formedness diagnostic `validate` used to
//! report).

use std::fs;
use std::path::Path;

use callisto_graph::commands::{status, StatusOptions};
use callisto_graph::infer::NoInference;
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::Workspace;
use callisto_model::{CommandError, CommandOutput, CommandRunner, DiagnosticCode, PackageId, Severity};

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

fn git_init_with_commit(root: &Path) {
    callisto_fixtures::git::init_repo(root);
    fs::write(root.join(".gitkeep"), "").unwrap();
    for args in [vec!["add", "."], vec!["commit", "-q", "-m", "init"]] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(root)
            .output()
            .expect("git must be installed");
    }
}

/// AC-01: a Fixed group's union bump must show up as `pending_severity` for
/// every member, including one with no changeset of its own naming it --
/// `status` must read this from `plan_version`'s cascade output, not from a
/// per-package changeset scan (which would show `None` for the un-named member).
#[test]
fn status_pending_severity_reflects_fixed_group_cascade() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    for name in ["pkg-a", "pkg-b"] {
        fs::create_dir_all(root.join(name)).unwrap();
        fs::write(
            root.join(name).join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
        )
        .unwrap();
    }
    fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"ab\"\nmembers = [\"pkg-a\", \"pkg-b\"]\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".changeset")).unwrap();
    // Only pkg-a is named directly; pkg-b's bump comes solely from the Fixed group union.
    fs::write(root.join(".changeset/bump.md"), "---\npkg-a: minor\n---\n\nfeature.\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-q", "-m", "add packages"])
        .current_dir(root)
        .output()
        .unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let report = status(&ws, &NoInference, &StatusOptions::default()).expect("status must succeed");

    let pkg_b = PackageId::parse("pkg-b").unwrap();
    let rec = report
        .packages
        .iter()
        .find(|p| p.package == pkg_b)
        .expect("pkg-b must be in the report");

    assert_eq!(
        rec.pending_severity,
        Some(Severity::Minor),
        "pkg-b's pending severity must reflect the Fixed group's union bump, \
         even though no changeset names it directly; got: {:?}",
        rec.pending_severity
    );
    assert!(
        rec.pending_changesets.is_empty(),
        "pkg-b has no changeset directly naming it, so pending_changesets stays empty \
         even though pending_severity is Some; got: {:?}",
        rec.pending_changesets
    );
}

/// AC-01: a package with no planned bump at all shows `None`, not a stale
/// leftover severity from a directly-named-but-inert changeset entry.
#[test]
fn status_pending_severity_is_none_with_no_planned_bump() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    fs::create_dir_all(root.join("pkg-a")).unwrap();
    fs::write(
        root.join("pkg-a/Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    fs::write(root.join("callisto.toml"), "").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-q", "-m", "add package"])
        .current_dir(root)
        .output()
        .unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let report = status(&ws, &NoInference, &StatusOptions::default()).expect("status must succeed");
    let rec = &report.packages[0];
    assert_eq!(rec.pending_severity, None, "no pending changeset means no bump");
}

/// AC-03: a changeset naming a package outside the workspace must surface as
/// `DiagnosticCode::UnknownPackage` on `status`, exactly as `validate` used to
/// report it, so `status --check` can gate on it.
#[test]
fn status_reports_unknown_package_diagnostic() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    fs::create_dir_all(root.join("pkg-a")).unwrap();
    fs::write(
        root.join("pkg-a/Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    fs::write(root.join("callisto.toml"), "").unwrap();
    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(
        root.join(".changeset/bad.md"),
        "---\ncargo/does-not-exist: patch\n---\n\nSome change.\n",
    )
    .unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-q", "-m", "add package"])
        .current_dir(root)
        .output()
        .unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let report = status(&ws, &NoInference, &StatusOptions::default()).expect("status must succeed");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::UnknownPackage),
        "expected an UnknownPackage diagnostic; got: {:?}",
        report.diagnostics
    );
}

/// AC-03: a changeset with entries but no package/severity lines still parses
/// as an entries-empty changeset (summary alone) -- `status` must report it as
/// `DiagnosticCode::EmptyChangeset`, exactly as `validate` used to.
#[test]
fn status_reports_empty_changeset_diagnostic() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    fs::write(root.join("callisto.toml"), "").unwrap();
    fs::create_dir_all(root.join(".changeset")).unwrap();
    // Frontmatter with zero entries but a non-empty summary parses successfully
    // (entries-empty + summary-empty is the only combination `parse_changeset`
    // rejects outright), giving `EmptyChangeset` a live path through `status`.
    fs::write(
        root.join(".changeset/bad.md"),
        "---\n---\n\nSome change with no entries.\n",
    )
    .unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-q", "-m", "add changeset"])
        .current_dir(root)
        .output()
        .unwrap();

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let report = status(&ws, &NoInference, &StatusOptions::default()).expect("status must succeed");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::EmptyChangeset),
        "expected an EmptyChangeset diagnostic; got: {:?}",
        report.diagnostics
    );
}
