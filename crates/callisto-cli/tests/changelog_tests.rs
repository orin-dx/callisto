mod common;

use std::fs;

use callisto_cli::cli::{AddArgs, GlobalArgs, InitArgs, OutputFormat, VersionArgs};
use callisto_cli::commands;

use common::setup_polyglot_git_repo;

/// Spec: `plan_version()` must use the real changeset summary text when generating
/// changelog entries, not the hardcoded placeholder "Release update".
///
/// This is a regression test for the bug where `plan_version()` ignored
/// `agg.changelog_inputs` and always emitted `summary: "Release update"`.
#[test]
fn test_changelog_content_uses_real_changeset_summary() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::init::handle(InitArgs { yes: true }, &global).unwrap();

    commands::add::handle(
        AddArgs {
            packages: vec!["core-crate:patch".to_string()],
            summary: Some("Fix the frobnication bug".to_string()),
        },
        &global,
    )
    .unwrap();

    commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            strict: false,
            strict_graph: false,
            allow_empty_changesets: false,
        },
        &global,
    )
    .unwrap();

    let changelog_path = root.join("crates/core/CHANGELOG.md");
    assert!(
        changelog_path.exists(),
        "crates/core/CHANGELOG.md must exist"
    );
    let changelog_content = fs::read_to_string(&changelog_path).unwrap();

    assert!(
        changelog_content.contains("Fix the frobnication bug"),
        "changelog must contain the real changeset summary 'Fix the frobnication bug', \
         but got:\n{changelog_content}"
    );
    assert!(
        !changelog_content.contains("Release update"),
        "changelog must NOT contain the hardcoded placeholder 'Release update', \
         but got:\n{changelog_content}"
    );
}

/// Spec: when multiple changesets both target the same package, all of their
/// summaries must appear in the generated changelog, not just one.
///
/// This is a regression test for the bug where multiple changesets were
/// collapsed to a single hardcoded entry.
#[test]
fn test_changelog_content_includes_all_changeset_summaries() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();

    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };

    commands::init::handle(InitArgs { yes: true }, &global).unwrap();

    commands::add::handle(
        AddArgs {
            packages: vec!["core-crate:patch".to_string()],
            summary: Some("Fix the widget alignment".to_string()),
        },
        &global,
    )
    .unwrap();

    commands::add::handle(
        AddArgs {
            packages: vec!["core-crate:patch".to_string()],
            summary: Some("Improve error messages".to_string()),
        },
        &global,
    )
    .unwrap();

    commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            strict: false,
            strict_graph: false,
            allow_empty_changesets: false,
        },
        &global,
    )
    .unwrap();

    let changelog_path = root.join("crates/core/CHANGELOG.md");
    assert!(
        changelog_path.exists(),
        "crates/core/CHANGELOG.md must exist"
    );
    let changelog_content = fs::read_to_string(&changelog_path).unwrap();

    assert!(
        changelog_content.contains("Fix the widget alignment"),
        "changelog must contain first changeset summary 'Fix the widget alignment', \
         but got:\n{changelog_content}"
    );
    assert!(
        changelog_content.contains("Improve error messages"),
        "changelog must contain second changeset summary 'Improve error messages', \
         but got:\n{changelog_content}"
    );
    assert!(
        !changelog_content.contains("Release update"),
        "changelog must NOT contain the hardcoded placeholder 'Release update', \
         but got:\n{changelog_content}"
    );
}
