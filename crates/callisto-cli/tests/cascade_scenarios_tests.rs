#![cfg(unix)]

//! Black-box cascade scenarios over the real `callisto` binary: dependencies a version bump
//! cannot silently round-trip must be rewritten faithfully or reported, never skipped quietly.

#[path = "common/release_harness.rs"]
mod release_harness;

use std::{fs, path::Path};

use release_harness::*;

/// A two-crate Cargo workspace where `app-crate` depends on `lib-crate` with `dependency`.
fn cargo_workspace(dependency: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/lib\", \"crates/app\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    for (name, extra) in [
        ("lib", String::new()),
        ("app", format!("\n[dependencies]\nlib-crate = {dependency}\n")),
    ] {
        fs::create_dir_all(root.join(format!("crates/{name}/src"))).unwrap();
        fs::write(
            root.join(format!("crates/{name}/Cargo.toml")),
            format!("[package]\nname = \"{name}-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{extra}"),
        )
        .unwrap();
        fs::write(root.join(format!("crates/{name}/src/lib.rs")), "pub fn f() {}\n").unwrap();
    }
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "workspace"]);
    let init = callisto(root, &["init", "--yes", "--versioning", "independent", "--no-workflow"]);
    assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "configure callisto"]);
    dir
}

fn add_changeset(root: &Path, package: &str, bump: &str) {
    let out = callisto(
        root,
        &["add", "--package", &format!("{package}:{bump}"), "--summary", "change"],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "changeset"]);
}

fn read(root: &Path, path: &str) -> String {
    fs::read_to_string(root.join(path)).unwrap()
}

#[test]
fn an_exact_pin_is_rewritten_in_place_and_its_dependent_released() {
    let dir = cargo_workspace("{ version = \"=0.1.0\", path = \"../lib\" }");
    let root = dir.path();
    add_changeset(root, "cargo/lib-crate", "minor");
    let out = callisto(root, &["version"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let app = read(root, "crates/app/Cargo.toml");
    assert!(
        app.contains("lib-crate = { version = \"=0.2.0\", path = \"../lib\" }"),
        "the exact pin must follow the dependency and keep the inline table intact:\n{app}"
    );
    assert!(
        app.contains("version = \"0.1.1\"\nedition"),
        "the dependent is patch-released:\n{app}"
    );
    assert!(read(root, "crates/lib/Cargo.toml").contains("version = \"0.2.0\""));
}

#[test]
fn a_range_the_bump_leaves_behind_and_cannot_be_rewritten_stops_the_run_before_any_write() {
    let dir = cargo_workspace("{ version = \">=0.1.0, <0.2.0\", path = \"../lib\" }");
    let root = dir.path();
    add_changeset(root, "cargo/lib-crate", "minor");
    let out = callisto(root, &["version"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.contains("E319"), "{stderr}");
    assert!(stderr.contains("crates/app/Cargo.toml"), "{stderr}");
    assert!(read(root, "crates/lib/Cargo.toml").contains("version = \"0.1.0\""));
    assert!(read(root, "crates/app/Cargo.toml").contains(">=0.1.0, <0.2.0"));
    assert!(
        root.join(".changeset").read_dir().unwrap().count() > 1,
        "the changeset stays"
    );
}

#[test]
fn a_linked_group_naming_a_package_outside_the_workspace_is_rejected_with_the_member_named() {
    let dir = cargo_workspace("{ version = \"=0.1.0\", path = \"../lib\" }");
    let root = dir.path();
    let config = read(root, "callisto.toml");
    fs::write(
        root.join("callisto.toml"),
        format!(
            "{config}\n[[linked-group]]\nname = \"pair\"\nmembers = [\"cargo/lib-crate\", \"cargo/ghost-crate\"]\n"
        ),
    )
    .unwrap();
    let out = callisto(root, &["status"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.contains("E108") && stderr.contains("ghost-crate"), "{stderr}");
}

/// A pnpm workspace where `app` takes `lib` through a `catalog:` entry.
fn pnpm_catalog_workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    fs::write(
        root.join("package.json"),
        "{\n  \"name\": \"root\",\n  \"private\": true\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("pnpm-workspace.yaml"),
        "packages:\n  - packages/*\ncatalog:\n  lib: ^0.1.0\n",
    )
    .unwrap();
    for (name, deps) in [
        ("lib", String::new()),
        (
            "app",
            ",\n  \"dependencies\": {\n    \"lib\": \"catalog:\"\n  }".to_owned(),
        ),
    ] {
        fs::create_dir_all(root.join(format!("packages/{name}"))).unwrap();
        fs::write(
            root.join(format!("packages/{name}/package.json")),
            format!("{{\n  \"name\": \"{name}\",\n  \"version\": \"0.1.0\"{deps}\n}}\n"),
        )
        .unwrap();
    }
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "workspace"]);
    let init = callisto(root, &["init", "--yes", "--versioning", "independent", "--no-workflow"]);
    assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "configure callisto"]);
    dir
}

/// An npm range written with spaces between its comparators is one range: once its dependency leaves it, the run
/// stops before any write, instead of skipping the dependent as if the range could not be read.
#[test]
fn a_space_separated_npm_range_the_bump_leaves_behind_stops_the_run_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    fs::write(
        root.join("package.json"),
        "{\n  \"name\": \"root\",\n  \"private\": true,\n  \"workspaces\": [\"packages/*\"]\n}\n",
    )
    .unwrap();
    for (name, extra) in [
        ("lib", String::new()),
        (
            "app",
            ",\n  \"peerDependencies\": {\n    \"lib\": \">=0.1.0 <1.0.0\"\n  }".to_owned(),
        ),
    ] {
        fs::create_dir_all(root.join(format!("packages/{name}"))).unwrap();
        fs::write(
            root.join(format!("packages/{name}/package.json")),
            format!("{{\n  \"name\": \"{name}\",\n  \"version\": \"0.1.0\"{extra}\n}}\n"),
        )
        .unwrap();
    }
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "workspace"]);
    let init = callisto(root, &["init", "--yes", "--no-workflow"]);
    assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "configure callisto"]);
    add_changeset(root, "npm/lib", "major");
    let before = read(root, "packages/app/package.json");

    let out = callisto(root, &["version", "--no-refresh-lockfiles"]);

    assert!(!out.status.success(), "the range must stop the run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("E319") && stderr.contains(">=0.1.0 <1.0.0"), "{stderr}");
    assert_eq!(read(root, "packages/app/package.json"), before, "nothing was written");
}

#[test]
fn a_catalog_dependency_is_reported_not_rewritten_and_does_not_cascade() {
    let dir = pnpm_catalog_workspace();
    let root = dir.path();
    add_changeset(root, "npm/lib", "minor");
    let out = callisto(root, &["version", "--dry-run"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let plan: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let diagnostics = plan["diagnostics"].as_array().expect("a catalog spec must be reported");
    let catalog = diagnostics
        .iter()
        .find(|d| d["code"] == "catalog-spec-not-rewritten")
        .unwrap_or_else(|| panic!("missing catalog diagnostic: {diagnostics:?}"));
    assert_eq!(catalog["package"], "npm/app");
    assert_eq!(catalog["path"], "packages/app/package.json");
    let bumped: Vec<&str> = plan["bumps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|bump| bump["package"].as_str().unwrap())
        .collect();
    assert_eq!(
        bumped,
        ["npm/lib"],
        "a spec that cannot be tested must not cascade a release"
    );
    assert!(read(root, "packages/app/package.json").contains("\"lib\": \"catalog:\""));
}

#[test]
fn every_cargo_dependency_table_is_rewritten_without_disturbing_the_rest_of_the_file() {
    let dir = cargo_workspace("{ version = \"=0.1.0\", path = \"../lib\" }");
    let root = dir.path();
    let app = "\
[package]
name = \"app-crate\"
version = \"0.1.0\"
edition = \"2021\"

# runtime dependency
[dependencies]
lib-crate = { version = \"=0.1.0\", path = \"../lib\" } # pinned to the sibling

[dev-dependencies]
lib-crate = { path = \"../lib\", version = \"=0.1.0\" }

[target.'cfg(unix)'.dependencies]
lib-crate = { version = \"=0.1.0\", path = \"../lib\" }
";
    fs::write(root.join("crates/app/Cargo.toml"), app).unwrap();
    git(root, &["commit", "-q", "-am", "dependency tables"]);
    add_changeset(root, "cargo/lib-crate", "minor");
    let out = callisto(root, &["version"]);
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stderr),
        read(root, "crates/app/Cargo.toml")
    );
    let expected = app.replace("=0.1.0", "=0.2.0").replace(
        "name = \"app-crate\"\nversion = \"0.1.0\"",
        "name = \"app-crate\"\nversion = \"0.1.1\"",
    );
    assert_eq!(
        read(root, "crates/app/Cargo.toml"),
        expected,
        "only the version values may change"
    );
}

#[test]
fn a_dependency_declared_only_for_a_target_still_cascades_to_its_dependent() {
    let dir = cargo_workspace("{ version = \"=0.1.0\", path = \"../lib\" }");
    let root = dir.path();
    let app = "\
[package]
name = \"app-crate\"
version = \"0.1.0\"
edition = \"2021\"

[target.'cfg(windows)'.dependencies]
lib-crate = { version = \"=0.1.0\", path = \"../lib\" }
";
    fs::write(root.join("crates/app/Cargo.toml"), app).unwrap();
    git(root, &["commit", "-q", "-am", "target-only dependency"]);
    add_changeset(root, "cargo/lib-crate", "minor");
    let plan = callisto(root, &["version", "--dry-run"]);
    assert!(plan.status.success(), "{}", String::from_utf8_lossy(&plan.stderr));
    let plan: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    let cascaded = plan["bumps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|bump| bump["package"] == "cargo/app-crate")
        .unwrap_or_else(|| panic!("a target-only dependent must be released with its dependency: {plan}"));
    assert_eq!(cascaded["reason"]["kind"], "cascade");
    assert_eq!(cascaded["reason"]["via"], "cargo/lib-crate");
}

#[test]
fn a_target_declaration_that_still_covers_the_new_version_has_its_floor_raised() {
    let dir = cargo_workspace("{ version = \"=0.1.0\", path = \"../lib\" }");
    let root = dir.path();
    let app = "\
[package]
name = \"app-crate\"
version = \"0.1.0\"
edition = \"2021\"

[dependencies]
lib-crate = { version = \"=0.1.0\", path = \"../lib\" }

[target.'cfg(windows)'.dependencies]
lib-crate = { version = \">=0.1.0, <0.3.0\", path = \"../lib\" }
";
    fs::write(root.join("crates/app/Cargo.toml"), app).unwrap();
    git(root, &["commit", "-q", "-am", "same crate, different constraint"]);
    add_changeset(root, "cargo/lib-crate", "minor");
    let out = callisto(root, &["version"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let after = read(root, "crates/app/Cargo.toml");
    assert!(
        after.contains("lib-crate = { version = \"=0.2.0\", path = \"../lib\" }"),
        "{after}"
    );
    assert!(
        after.contains("lib-crate = { version = \">=0.2.0, <0.3.0\", path = \"../lib\" }"),
        "a released package raises the floor of every declaration, including one that still covers the new version:\n{after}"
    );
}

#[test]
fn a_dependency_declared_twice_with_different_constraints_has_each_declaration_rewritten() {
    let dir = cargo_workspace("{ version = \"=0.1.0\", path = \"../lib\" }");
    let root = dir.path();
    let app = "\
[package]
name = \"app-crate\"
version = \"0.1.0\"
edition = \"2021\"

[dependencies]
lib-crate = { version = \"^0.1.0\", path = \"../lib\" }

# only on unix
[target.'cfg(unix)'.dependencies]
lib-crate = { version = \"=0.1.0\", path = \"../lib\" } # exact on unix
";
    fs::write(root.join("crates/app/Cargo.toml"), app).unwrap();
    git(root, &["commit", "-q", "-am", "one crate, two constraints"]);
    add_changeset(root, "cargo/lib-crate", "minor");
    let out = callisto(root, &["version"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let expected = "\
[package]
name = \"app-crate\"
version = \"0.1.1\"
edition = \"2021\"

[dependencies]
lib-crate = { version = \"^0.2.0\", path = \"../lib\" }

# only on unix
[target.'cfg(unix)'.dependencies]
lib-crate = { version = \"=0.2.0\", path = \"../lib\" } # exact on unix
";
    assert_eq!(read(root, "crates/app/Cargo.toml"), expected);
}
