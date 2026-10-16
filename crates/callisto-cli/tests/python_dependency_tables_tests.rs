#![cfg(unix)]

//! Black-box tests that Python dependencies declared as published extras
//! (`[project.optional-dependencies]`) are tracked like runtime ones: a bump releases the
//! package that pins it and rewrites the pin, instead of leaving published metadata stale.

#[path = "common/release_harness.rs"]
mod release_harness;

use std::{fs, path::Path};

use release_harness::*;

fn python_workspace(app_pyproject: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    fs::write(
        root.join("pyproject.toml"),
        "[tool.uv.workspace]\nmembers = [\"packages/*\"]\n",
    )
    .unwrap();
    for (name, body) in [
        ("lib", "[project]\nname = \"lib-py\"\nversion = \"0.1.0\"\n"),
        ("app", app_pyproject),
    ] {
        fs::create_dir_all(root.join(format!("packages/{name}"))).unwrap();
        fs::write(root.join(format!("packages/{name}/pyproject.toml")), body).unwrap();
    }
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "workspace"]);
    let init = callisto(root, &["init", "--yes", "--versioning", "independent", "--no-workflow"]);
    assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "configure callisto"]);
    dir
}

fn bump_lib_minor(root: &Path) {
    let out = callisto(root, &["add", "--package", "lib-py:minor", "--summary", "change"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "changeset"]);
    let out = callisto(root, &["version"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

fn app_pyproject(root: &Path) -> String {
    fs::read_to_string(root.join("packages/app/pyproject.toml")).unwrap()
}

#[test]
fn a_pin_in_an_extra_cascades_and_is_rewritten_in_place() {
    let dir = python_workspace(
        "[project]\nname = \"app-py\"\nversion = \"0.1.0\"\n\n# published extras\n[project.optional-dependencies]\nfast = [\"lib-py==0.1.0\"]\n",
    );
    bump_lib_minor(dir.path());
    assert_eq!(
        app_pyproject(dir.path()),
        "[project]\nname = \"app-py\"\nversion = \"0.1.1\"\n\n# published extras\n[project.optional-dependencies]\nfast = [\"lib-py==0.2.0\"]\n",
        "the extra follows the dependency, the dependent is patch-released, nothing else changes"
    );
}

#[test]
fn a_pin_repeated_in_runtime_dependencies_and_an_extra_is_rewritten_in_both() {
    let dir = python_workspace(
        "[project]\nname = \"app-py\"\nversion = \"0.1.0\"\ndependencies = [\"lib-py==0.1.0\"]\n\n[project.optional-dependencies]\nfast = [\"lib-py==0.1.0\"]\n",
    );
    bump_lib_minor(dir.path());
    let after = app_pyproject(dir.path());
    assert_eq!(after.matches("lib-py==0.2.0").count(), 2, "{after}");
    assert!(!after.contains("lib-py==0.1.0"), "{after}");
}

#[test]
fn an_extra_range_that_still_covers_the_new_version_has_its_floor_raised_like_a_plain_requirement() {
    let dir = python_workspace(
        "[project]\nname = \"app-py\"\nversion = \"0.1.0\"\n\n[project.optional-dependencies]\nfast = [\"lib-py==0.1.0\"]\nwide = [\"lib-py>=0.1,<0.3\"]\n",
    );
    bump_lib_minor(dir.path());
    let after = app_pyproject(dir.path());
    assert!(after.contains("fast = [\"lib-py==0.2.0\"]"), "{after}");
    assert!(
        after.contains("wide = [\"lib-py>=0.2.0, <1\"]"),
        "the range is rewritten by its own constraint, never the pin's:\n{after}"
    );
}

#[test]
fn a_loose_extra_does_not_hold_back_a_stale_pin_in_another_extra() {
    let dir = python_workspace(
        "[project]\nname = \"app-py\"\nversion = \"0.1.0\"\n\n[project.optional-dependencies]\na = [\"lib-py>=0.1\"]\nc = [\"lib-py==0.1.0\"]\n",
    );
    bump_lib_minor(dir.path());
    let after = app_pyproject(dir.path());
    assert!(
        after.contains("c = [\"lib-py==0.2.0\"]"),
        "the stale pin follows the bump:\n{after}"
    );
    assert!(!after.contains("lib-py==0.1.0"), "{after}");
    assert!(
        after.contains("a = [\"lib-py>=0.2.0\"]"),
        "the loose extra gets its floor raised by its own constraint, not the pin's:\n{after}"
    );
}

#[test]
fn a_stale_pin_listed_before_a_loose_extra_is_still_rewritten_alone() {
    let dir = python_workspace(
        "[project]\nname = \"app-py\"\nversion = \"0.1.0\"\n\n[project.optional-dependencies]\nc = [\"lib-py==0.1.0\"]\na = [\"lib-py>=0.1\"]\n",
    );
    bump_lib_minor(dir.path());
    let after = app_pyproject(dir.path());
    assert!(after.contains("c = [\"lib-py==0.2.0\"]"), "{after}");
    assert!(
        after.contains("a = [\"lib-py>=0.2.0\"]"),
        "the loose extra keeps its own shape:\n{after}"
    );
}

#[test]
fn every_marker_variant_of_a_requirement_is_rewritten_with_its_marker() {
    let dir = python_workspace(
        "[project]\nname = \"app-py\"\nversion = \"0.1.0\"\ndependencies = [\n  \"lib-py==0.1.0; python_version<'3.9'\",\n  \"lib-py==0.1.0; python_version>='3.9'\",\n]\n",
    );
    bump_lib_minor(dir.path());
    let after = app_pyproject(dir.path());
    assert!(after.contains("\"lib-py==0.2.0;python_version<'3.9'\""), "{after}");
    assert!(after.contains("\"lib-py==0.2.0;python_version>='3.9'\""), "{after}");
    assert!(!after.contains("lib-py==0.1.0"), "{after}");
}

#[test]
fn runtime_dependencies_with_different_constraints_are_each_rewritten_by_their_own() {
    let dir = python_workspace(
        "[project]\nname = \"app-py\"\nversion = \"0.1.0\"\ndependencies = [\"lib-py>=0.1; python_version>='3.9'\", \"lib-py==0.1.0; python_version<'3.9'\"]\n",
    );
    bump_lib_minor(dir.path());
    let after = app_pyproject(dir.path());
    assert!(after.contains("lib-py==0.2.0;python_version<'3.9'"), "{after}");
    assert!(after.contains("lib-py>=0.2.0;python_version>='3.9'"), "{after}");
    assert!(!after.contains("lib-py==0.1.0"), "{after}");
}

#[test]
fn a_poetry_dependency_repeated_in_project_dependencies_and_an_extra_is_rewritten_in_each() {
    let dir = python_workspace(
        "[project]\nname = \"app-py\"\nversion = \"0.1.0\"\ndependencies = [\"lib-py==0.1.0\"]\n\n[project.optional-dependencies]\nfast = [\"lib-py==0.1.0\"]\n\n[tool.poetry.dependencies]\nlib-py = \"==0.1.0\"\n",
    );
    bump_lib_minor(dir.path());
    let after = app_pyproject(dir.path());
    assert_eq!(after.matches("lib-py==0.2.0").count(), 2, "{after}");
    assert!(after.contains("lib-py = \"==0.2.0\""), "{after}");
    assert!(!after.contains("0.1.0\""), "{after}");
}

#[test]
fn requirement_spellings_and_extras_syntax_are_matched_by_normalized_name() {
    let dir = python_workspace(
        "[project]\nname = \"app-py\"\nversion = \"0.1.0\"\ndependencies = [\"Lib_Py[x]==0.1.0\", \"lib.py==0.1.0\"]\n",
    );
    bump_lib_minor(dir.path());
    let after = app_pyproject(dir.path());
    assert!(after.contains("\"Lib_Py[x]==0.2.0\""), "{after}");
    assert!(after.contains("\"lib.py==0.2.0\""), "{after}");
}
