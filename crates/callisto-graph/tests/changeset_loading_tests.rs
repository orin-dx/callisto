//! `load_changesets` parse-time error behavior -- split out of
//! validate_command_tests.rs when `validate` was removed.
//! These test `load_changesets` itself, not the removed `validate` command.

#[test]
fn test_load_changesets_detects_empty_changesets() {
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
fn test_load_changesets_rejects_entries_with_empty_summary() {
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
                callisto_model::format::ParseError::EmptySummary,
                "expected EmptySummary parse error"
            );
        }
        other => panic!("expected ParseChangeset error, got {other:?}"),
    }
}

/// A changeset file with entries and a whitespace-only summary must also be rejected.
#[test]
fn test_load_changesets_rejects_entries_with_whitespace_only_summary() {
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
            assert_eq!(source, callisto_model::format::ParseError::EmptySummary);
        }
        other => panic!("expected ParseChangeset error, got {other:?}"),
    }
}
