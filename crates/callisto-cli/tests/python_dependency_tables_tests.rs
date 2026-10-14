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
fn an_extra_with_a_different_constraint_is_left_as_written() {
    let dir = python_workspace(
        "[project]\nname = \"app-py\"\nversion = \"0.1.0\"\n\n[project.optional-dependencies]\nfast = [\"lib-py==0.1.0\"]\nwide = [\"lib-py>=0.1,<0.3\"]\n",
    );
    bump_lib_minor(dir.path());
    let after = app_pyproject(dir.path());
    assert!(after.contains("fast = [\"lib-py==0.2.0\"]"), "{after}");
    assert!(
        after.contains("wide = [\"lib-py>=0.1,<0.3\"]"),
        "a range that still covers the new version stays as written:\n{after}"
    );
}
