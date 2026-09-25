mod common;

use callisto_cli::cli::{AddArgs, GlobalArgs, OutputFormat, VersionArgs};
use callisto_cli::commands;

use common::setup_polyglot_git_repo;

/// Regression test: outside pre mode, `callisto add` followed by `callisto
/// version` with no commit in between, then `version` run again with still
/// nothing committed, is on-disk indistinguishable from a crash-then-rerun
/// (a canonical manifest already bumped and staged, but never committed).
/// `check_partial_run` must refuse the second run (E121) instead of silently
/// stacking a second bump/changelog entry on top of the first uncommitted one.
#[test]
fn test_version_twice_without_commit_is_refused_as_partial_run() {
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
            allow_empty_changesets: false,
            emit_decision: None,
        },
        &global,
    )
    .expect("the first version run must succeed and stage its changes");

    let second = commands::version::handle(
        VersionArgs {
            refresh_lockfiles: false,
            strict: false,
            allow_empty_changesets: false,
            emit_decision: None,
        },
        &global,
    );

    assert!(
        second.is_err(),
        "a second `version` run with nothing committed since the first must be refused, not \
         silently apply a second bump on top of the first uncommitted one"
    );
    let message = second.unwrap_err().to_string();
    assert!(
        message.contains("this looks like a `version` run that wrote and staged its changes but never committed")
            || message.contains("E121"),
        "expected the partial-run (E121) refusal, got: {message}"
    );
}
