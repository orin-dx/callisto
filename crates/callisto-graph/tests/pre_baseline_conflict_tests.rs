//! A legacy `pre.json` key and its package's canonical key must not silently disagree about the baseline.
#![allow(clippy::result_large_err)]

use std::fs;
use std::path::Path;

use callisto_fixtures::git::{init_repo, run_git, GitRunner};
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::{GraphError, Workspace};
use callisto_model::format::{parse_pre_json, PreState};

fn commit(root: &Path, message: &str) {
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", message]);
}

fn pre_json(initial: &[(&str, &str)]) -> String {
    let versions: serde_json::Map<String, serde_json::Value> = initial
        .iter()
        .map(|(key, version)| ((*key).to_string(), serde_json::json!(version)))
        .collect();
    serde_json::json!({"mode": "pre", "tag": "beta", "initialVersions": versions, "changesets": []}).to_string()
}

/// A Cargo or PyPI `foo` that existed when `pre.json` was added, then an npm `foo` joined it.
fn promoted_workspace(pypi: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_repo(root);
    fs::write(root.join("callisto.toml"), "# callisto configuration\n").unwrap();
    fs::create_dir_all(root.join("native")).unwrap();
    if pypi {
        fs::write(
            root.join("native/pyproject.toml"),
            "[project]\nname = \"foo\"\nversion = \"1.2.3\"\n",
        )
        .unwrap();
    } else {
        fs::write(
            root.join("native/Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("native/src")).unwrap();
        fs::write(root.join("native/src/lib.rs"), "\n").unwrap();
    }
    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(root.join(".changeset/pre.json"), "{}").unwrap();
    commit(root, "native foo");
    fs::create_dir_all(root.join("web")).unwrap();
    fs::write(
        root.join("web/package.json"),
        "{\"name\":\"foo\",\"version\":\"3.0.0\"}\n",
    )
    .unwrap();
    commit(root, "npm foo");
    dir
}

fn resolve(pypi: bool, initial: &[(&str, &str)]) -> Result<PreState, GraphError> {
    let dir = promoted_workspace(pypi);
    let text = pre_json(initial);
    let state = parse_pre_json(&text).unwrap();
    let ws = Workspace::load(
        dir.path().to_path_buf(),
        &IgnoreWalkLocator::new(dir.path()),
        &GitRunner,
    )
    .unwrap();
    ws.resolve_pre_state_from_json(state, &text)
}

fn versions(state: &PreState) -> Vec<(String, String)> {
    state
        .initial_versions
        .iter()
        .map(|(key, version)| (key.clone(), version.render().to_string()))
        .collect()
}

fn pair(key: &str, version: &str) -> (String, String) {
    (key.to_string(), version.to_string())
}

fn conflict(error: GraphError) -> [(String, String); 2] {
    match error {
        GraphError::ConflictingPreJsonBaselines {
            key,
            version,
            other_key,
            other_version,
        } => [
            (key, version.render().to_string()),
            (other_key, other_version.render().to_string()),
        ],
        other => panic!("expected ConflictingPreJsonBaselines, got {other:?}"),
    }
}

#[test]
fn differing_legacy_and_canonical_versions_fail_naming_both_keys_and_versions() {
    let error = resolve(false, &[("foo", "1.0.0"), ("cargo/foo", "0.9.0")]).unwrap_err();
    assert_eq!(conflict(error), [pair("foo", "1.0.0"), pair("cargo/foo", "0.9.0")]);
}

#[test]
fn the_conflict_diagnostic_carries_a_code_and_a_fix() {
    use miette::Diagnostic;
    let error = resolve(false, &[("foo", "1.0.0"), ("cargo/foo", "0.9.0")]).unwrap_err();
    assert_eq!(error.code().unwrap().to_string(), "E305");
    let help = error.help().unwrap().to_string();
    assert!(help.contains("`foo`") && help.contains("`cargo/foo`"), "{help}");
    let message = error.to_string();
    for needle in ["`foo`", "`cargo/foo`", "1.0.0", "0.9.0"] {
        assert!(message.contains(needle), "{message}");
    }
}

#[test]
fn equal_legacy_and_canonical_versions_coalesce_under_the_canonical_key() {
    let state = resolve(false, &[("foo", "1.0.0"), ("cargo/foo", "1.0.0")]).unwrap();
    assert_eq!(versions(&state), vec![pair("cargo/foo", "1.0.0")]);
}

#[test]
fn a_legacy_key_with_no_canonical_key_migrates() {
    let state = resolve(false, &[("foo", "1.0.0"), ("unrelated", "4.0.0")]).unwrap();
    assert_eq!(
        versions(&state),
        vec![pair("unrelated", "4.0.0"), pair("cargo/foo", "1.0.0")]
    );
}

#[test]
fn prerelease_baselines_that_differ_conflict_and_equal_ones_coalesce() {
    let error = resolve(false, &[("foo", "1.0.0-beta.1"), ("cargo/foo", "1.0.0-beta.2")]).unwrap_err();
    assert_eq!(
        conflict(error),
        [pair("foo", "1.0.0-beta.1"), pair("cargo/foo", "1.0.0-beta.2")]
    );
    let state = resolve(false, &[("foo", "1.0.0-beta.1"), ("cargo/foo", "1.0.0-beta.1")]).unwrap();
    assert_eq!(versions(&state), vec![pair("cargo/foo", "1.0.0-beta.1")]);
}

#[test]
fn a_prerelease_and_its_release_are_different_baselines() {
    let error = resolve(false, &[("foo", "1.0.0"), ("cargo/foo", "1.0.0-beta.1")]).unwrap_err();
    assert!(matches!(error, GraphError::ConflictingPreJsonBaselines { .. }));
}

#[test]
fn python_baselines_differing_in_a_prerelease_number_conflict() {
    let error = resolve(true, &[("foo", "1.2.3b1"), ("pypi/foo", "1.2.3b2")]).unwrap_err();
    assert_eq!(conflict(error), [pair("foo", "1.2.3b1"), pair("pypi/foo", "1.2.3b2")]);
}

#[test]
fn python_baselines_in_different_spellings_of_one_version_coalesce_to_the_canonical_spelling() {
    for legacy in ["1.2.3-beta.1", "1.2.3_beta1", "1.2.3B1", "1.2.3.0b1"] {
        let state = resolve(true, &[("foo", legacy), ("pypi/foo", "1.2.3b1")]).unwrap();
        assert_eq!(
            versions(&state),
            vec![pair("pypi/foo", "1.2.3b1")],
            "legacy spelling {legacy}"
        );
    }
}

#[test]
fn two_legacy_spellings_with_no_canonical_key_must_agree() {
    let error = resolve(true, &[("foo", "1.2.3"), ("pypi:foo", "1.2.4")]).unwrap_err();
    assert_eq!(conflict(error), [pair("pypi:foo", "1.2.4"), pair("foo", "1.2.3")]);
    let state = resolve(true, &[("foo", "1.2.3"), ("pypi:foo", "1.2.3")]).unwrap();
    assert_eq!(versions(&state), vec![pair("pypi/foo", "1.2.3")]);
}

#[test]
fn three_keys_for_one_package_conflict_unless_all_agree() {
    let state = resolve(true, &[("foo", "1.2.3"), ("pypi:foo", "1.2.3"), ("pypi/foo", "1.2.3")]).unwrap();
    assert_eq!(versions(&state), vec![pair("pypi/foo", "1.2.3")]);

    let error = resolve(true, &[("foo", "1.2.3"), ("pypi:foo", "1.2.4"), ("pypi/foo", "1.2.3")]).unwrap_err();
    assert_eq!(conflict(error), [pair("pypi:foo", "1.2.4"), pair("pypi/foo", "1.2.3")]);

    let error = resolve(true, &[("foo", "1.2.3"), ("pypi:foo", "1.2.3"), ("pypi/foo", "1.2.4")]).unwrap_err();
    assert_eq!(conflict(error), [pair("foo", "1.2.3"), pair("pypi/foo", "1.2.4")]);
}
