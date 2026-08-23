mod fixtures;
use fixtures::GraphBuilder;
use std::cell::OnceCell;

#[test]
fn test_validate_detects_empty_changesets() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cs_dir = temp_dir.path().join(".changeset");
    std::fs::create_dir_all(&cs_dir).unwrap();
    std::fs::write(cs_dir.join("empty.md"), "---\n---\n").unwrap();

    let cfg = callisto_graph::config::load(&temp_dir.path().join("callisto.toml")).unwrap();
    let loaded = callisto_graph::load_changesets(temp_dir.path(), &cfg);
    assert!(loaded.is_err());
}

/// A changeset file that has entries but an empty summary body must be rejected when
/// loaded, producing a ParseChangeset error wrapping ParseError::EmptySummary.
/// This catches the bug where entries+empty-summary was silently accepted and produced
/// a version bump with no changelog entry.
#[test]
fn test_validate_rejects_entries_with_empty_summary() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cs_dir = temp_dir.path().join(".changeset");
    std::fs::create_dir_all(&cs_dir).unwrap();
    // Entries present, summary body is empty
    std::fs::write(cs_dir.join("bad.md"), "---\ncargo/foo: patch\n---\n\n").unwrap();

    let cfg = callisto_graph::config::load(&temp_dir.path().join("callisto.toml")).unwrap();
    let loaded = callisto_graph::load_changesets(temp_dir.path(), &cfg);
    assert!(
        loaded.is_err(),
        "changeset with entries but empty summary must fail to load"
    );
    match loaded.unwrap_err() {
        callisto_graph::GraphError::ParseChangeset { source, .. } => {
            assert_eq!(
                source,
                callisto_format::ParseError::EmptySummary,
                "expected EmptySummary parse error"
            );
        }
        other => panic!("expected ParseChangeset error, got {other:?}"),
    }
}

/// A changeset file with entries and a whitespace-only summary must also be rejected.
#[test]
fn test_validate_rejects_entries_with_whitespace_only_summary() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cs_dir = temp_dir.path().join(".changeset");
    std::fs::create_dir_all(&cs_dir).unwrap();
    std::fs::write(cs_dir.join("bad.md"), "---\ncargo/foo: minor\n---\n\n  \t  \n\n").unwrap();

    let cfg = callisto_graph::config::load(&temp_dir.path().join("callisto.toml")).unwrap();
    let loaded = callisto_graph::load_changesets(temp_dir.path(), &cfg);
    assert!(
        loaded.is_err(),
        "changeset with entries but whitespace-only summary must fail to load"
    );
    match loaded.unwrap_err() {
        callisto_graph::GraphError::ParseChangeset { source, .. } => {
            assert_eq!(source, callisto_format::ParseError::EmptySummary);
        }
        other => panic!("expected ParseChangeset error, got {other:?}"),
    }
}

#[test]
fn test_validate_since_git_diff_argument_ordering() {
    use callisto_graph::commands::{validate, ValidateOptions};
    use callisto_model::{CommandOutput, CommandRunner};
    use std::sync::atomic::{AtomicBool, Ordering};

    static CALLED_CORRECTLY: AtomicBool = AtomicBool::new(false);

    struct ValidateArgTestRunner;
    impl CommandRunner for ValidateArgTestRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<CommandOutput, callisto_model::CommandError> {
            if program == "git"
                && args.len() >= 4
                && args[0] == "diff"
                && args[1] == "--name-only"
                && args[2] == "main..HEAD"
                && args[3] == "--"
            {
                CALLED_CORRECTLY.store(true, Ordering::SeqCst);
            }
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: "".to_string(),
                stderr: "".to_string(),
            })
        }
    }

    let runner = ValidateArgTestRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    let cfg = callisto_graph::config::load(&ws_dir.path().join("callisto.toml")).unwrap();
    let graph = GraphBuilder::new().build().unwrap();
    let git = callisto_vcs::GitAccess::discover(ws_dir.path(), &runner);
    let tags = callisto_graph::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: ws_dir.path().to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        git: OnceCell::from(git),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let opts = ValidateOptions {
        staged: false,
        since: Some("main".to_string()),
        strict: false,
        strict_graph: false,
    };

    let _res = validate(&ws, &opts);
    assert!(
        CALLED_CORRECTLY.load(Ordering::SeqCst),
        "git diff argument ordering must be `git diff --name-only main..HEAD --`"
    );
}

/// Spec: `validate` must return `ok: false` and emit an Error-severity diagnostic when a
/// changeset entry has a package name that passes the frontmatter parser (raw string storage)
/// but is rejected by `PackageId::parse` — for example, a name starting with `-`. Previously
/// the `if let Ok(id)` guard silently dropped the malformed entry, so the report came back
/// `ok: true` with no diagnostics at all.
#[test]
fn test_validate_detects_malformed_package_name_in_changeset() {
    use callisto_graph::commands::{validate, ValidateOptions};
    use callisto_model::DiagnosticSeverity;

    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();
    let cs_dir = root.join(".changeset");
    std::fs::create_dir_all(&cs_dir).unwrap();

    // `-invalid-pkg` passes parse_changeset (raw name stored as-is) but fails
    // PackageId::parse because it starts with `-` (PathTraversal rejection).
    std::fs::write(
        cs_dir.join("bad-name.md"),
        "---\n-invalid-pkg: patch\n---\n\nSummary.\n",
    )
    .unwrap();

    struct DummyRunner;
    impl callisto_model::CommandRunner for DummyRunner {
        fn run(
            &self,
            _program: &str,
            _args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            Ok(callisto_model::CommandOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    let runner = DummyRunner;
    let cfg = callisto_graph::config::load(root).unwrap();
    let graph = GraphBuilder::new().build().unwrap();
    let git = callisto_vcs::GitAccess::discover(root, &runner);
    let tags = callisto_graph::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        git: OnceCell::from(git),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let opts = ValidateOptions {
        staged: false,
        since: None,
        strict: false,
        strict_graph: false,
    };

    let report = validate(&ws, &opts).expect("validate must not error for a parseable changeset file");

    assert!(
        !report.ok,
        "validate must return ok: false for a changeset containing the malformed package name \
         `-invalid-pkg`; got ok: true with diagnostics: {:?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.severity == DiagnosticSeverity::Error && d.message.contains("-invalid-pkg")),
        "diagnostics must contain an Error-severity entry mentioning the invalid name; \
         got: {:?}",
        report.diagnostics
    );
}
