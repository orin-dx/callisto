mod common;

use std::fs;

use callisto_cli::cli::{AddArgs, GlobalArgs, OutputFormat, VersionArgs};
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

    callisto_fixtures::scaffold_callisto(&global.cwd);

    commands::add::handle(
        AddArgs {
            packages: vec!["cargo/core-crate:patch".to_string()],
            summary: Some("Fix the frobnication bug".to_string()),
        },
        &global,
    )
    .unwrap();

    commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            no_refresh_lockfiles: true,
            strict: false,
            allow_empty_changesets: false,
            emit_decision: None,
            emit_preview: None,
        },
        &global,
    )
    .unwrap();

    let changelog_path = root.join("crates/core/CHANGELOG.md");
    assert!(changelog_path.exists(), "crates/core/CHANGELOG.md must exist");
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

    callisto_fixtures::scaffold_callisto(&global.cwd);

    commands::add::handle(
        AddArgs {
            packages: vec!["cargo/core-crate:patch".to_string()],
            summary: Some("Fix the widget alignment".to_string()),
        },
        &global,
    )
    .unwrap();

    commands::add::handle(
        AddArgs {
            packages: vec!["cargo/core-crate:patch".to_string()],
            summary: Some("Improve error messages".to_string()),
        },
        &global,
    )
    .unwrap();

    commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            no_refresh_lockfiles: true,
            strict: false,
            allow_empty_changesets: false,
            emit_decision: None,
            emit_preview: None,
        },
        &global,
    )
    .unwrap();

    let changelog_path = root.join("crates/core/CHANGELOG.md");
    assert!(changelog_path.exists(), "crates/core/CHANGELOG.md must exist");
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

fn version_with_config(config: Option<&str>) -> String {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();
    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };
    callisto_fixtures::scaffold_callisto(&global.cwd);
    if let Some(config) = config {
        fs::write(root.join("callisto.toml"), config).unwrap();
    }
    commands::add::handle(
        AddArgs {
            packages: vec!["cargo/core-crate:patch".to_string()],
            summary: Some("Fix the frobnication bug".to_string()),
        },
        &global,
    )
    .unwrap();
    commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            no_refresh_lockfiles: true,
            strict: false,
            allow_empty_changesets: false,
            emit_decision: None,
            emit_preview: None,
        },
        &global,
    )
    .unwrap();
    fs::read_to_string(root.join("crates/core/CHANGELOG.md")).unwrap()
}

/// By default a section is grouped by bump level and carries today's date; no repository is configured, so no link.
#[test]
fn test_changelog_default_layout_is_grouped_with_a_date() {
    let content = version_with_config(None);
    let lines: Vec<&str> = content.lines().collect();
    let heading = lines
        .iter()
        .position(|l| l.starts_with("## "))
        .expect("a version heading");
    let date_line = lines[heading + 2];
    let date = date_line
        .strip_prefix('_')
        .and_then(|l| l.strip_suffix('_'))
        .expect(date_line);
    let bytes = date.as_bytes();
    assert!(
        date.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && date.bytes().filter(u8::is_ascii_digit).count() == 8,
        "expected a YYYY-MM-DD date line, got {date_line:?}"
    );
    assert_eq!(lines[heading + 4], "### Patch Changes", "{content}");
    assert_eq!(lines[heading + 6], "- Fix the frobnication bug", "{content}");
}

/// `layout = "flat"` writes the section exactly as before layouts existed.
#[test]
fn test_changelog_flat_layout_keeps_the_old_format() {
    let content = version_with_config(Some("[changelog]\nlayout = \"flat\"\n"));
    let after_title = content.split_once("\n\n").expect("a title then a section").1;
    let version = after_title.lines().next().unwrap();
    assert_eq!(
        after_title,
        format!("{version}\n\n- Fix the frobnication bug\n\n"),
        "{content}"
    );
}
