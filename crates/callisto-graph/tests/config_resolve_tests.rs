/// Tests that `[[linked-group]]` and `[[fixed-group]]` blocks in callisto.toml are
/// resolved into the workspace's `config.groups` table.
///
/// Regression test for the bug where `GroupTable::resolve()` was never called —
/// `raw_groups` was validated syntactically but then dropped, and
/// `ResolvedConfig.groups` was always set to `GroupTable::default()`.
use std::fs;
use std::path::Path;

use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::Workspace;
use callisto_model::{CommandError, CommandOutput, CommandRunner};

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

/// Set up a minimal two-package Cargo workspace with git init.
fn write_two_crate_workspace(root: &Path, crate_a: &str, crate_b: &str) {
    callisto_fixtures::git::init_repo(root);

    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    )
    .unwrap();

    for name in [crate_a, crate_b] {
        let crate_dir = root.join(format!("crates/{name}"));
        fs::create_dir_all(&crate_dir).unwrap();
        fs::write(
            crate_dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
    }
}

/// One directory holding both a Cargo and an npm manifest named `name`, so the package has two native names.
fn write_dual_package(root: &Path, name: &str) {
    callisto_fixtures::git::init_repo(root);
    let dir = root.join(format!("packages/{name}"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    )
    .unwrap();
    fs::write(
        dir.join("package.json"),
        format!(r#"{{"name":"{name}","version":"0.1.0"}}"#),
    )
    .unwrap();
}

/// A `[[linked-group]]` defined in `callisto.toml` must appear in
/// `ws.config.groups.linked` after `Workspace::load`.
///
/// Before the fix this assertion failed because `GroupTable::resolve` was
/// never called and `groups` was always `GroupTable::default()` (empty maps).
#[test]
fn linked_group_is_resolved_into_config() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_two_crate_workspace(root, "alpha", "beta");

    fs::write(
        root.join("callisto.toml"),
        r#"
[[linked-group]]
name = "ab"
members = ["cargo/alpha", "cargo/beta"]
"#,
    )
    .unwrap();

    let runner = NoopRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace should load");

    assert!(
        ws.config.groups.linked.keys().any(|k| k.as_str() == "ab"),
        "expected linked group 'ab' to be present, but groups.linked was empty — \
         GroupTable::resolve was not called (bug)"
    );
}

/// A `[[fixed-group]]` defined in `callisto.toml` must appear in
/// `ws.config.groups.fixed` after `Workspace::load`.
#[test]
fn fixed_group_is_resolved_into_config() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_two_crate_workspace(root, "foo", "bar");

    fs::write(
        root.join("callisto.toml"),
        r#"
[[fixed-group]]
name = "fb"
members = ["cargo/foo", "cargo/bar"]
"#,
    )
    .unwrap();

    let runner = NoopRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace should load");

    assert!(
        ws.config.groups.fixed.keys().any(|k| k.as_str() == "fb"),
        "expected fixed group 'fb' to be present, but groups.fixed was empty — \
         GroupTable::resolve was not called (bug)"
    );
}

/// Two Fixed groups' members that resolve to the
/// SAME PackageId under different spellings must make Workspace::load
/// (which calls GroupTable::resolve) fail with
/// GraphError::ConflictingGroupMembership -- not silently let the second
/// group's insert overwrite the first's claim.
#[test]
fn conflicting_fixed_group_membership_across_spellings_is_rejected() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_dual_package(root, "my-lib");

    fs::write(
        root.join("callisto.toml"),
        r#"
[[fixed-group]]
name = "group-a"
members = ["cargo/my-lib"]

[[fixed-group]]
name = "group-b"
members = ["npm/my-lib"]
"#,
    )
    .unwrap();

    let runner = NoopRunner;
    let locator = IgnoreWalkLocator::new(root);
    let result = Workspace::load(root.to_path_buf(), &locator, &runner);

    match result {
        Err(callisto_graph::error::GraphError::ConflictingGroupMembership { groups, .. }) => {
            let names: Vec<String> = groups.iter().map(|g| g.as_str().to_string()).collect();
            assert!(names.contains(&"group-a".to_string()));
            assert!(names.contains(&"group-b".to_string()));
        }
        Err(other) => panic!("expected Err(ConflictingGroupMembership) naming group-a and group-b, got Err({other:?})"),
        Ok(_) => panic!("expected Err(ConflictingGroupMembership) naming group-a and group-b, got Ok(_)"),
    }
}

/// A resolved PackageId listed as a Fixed-group member under one
/// spelling and a Linked-group member under a different spelling must also
/// be rejected -- confirming the conflict check spans both group kinds and
/// is not scoped separately to fixed_of and linked_of.
#[test]
fn conflicting_membership_across_fixed_and_linked_groups_is_rejected() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_dual_package(root, "my-lib");

    fs::write(
        root.join("callisto.toml"),
        r#"
[[fixed-group]]
name = "fixed-a"
members = ["cargo/my-lib"]

[[linked-group]]
name = "linked-b"
members = ["npm/my-lib"]
"#,
    )
    .unwrap();

    let runner = NoopRunner;
    let locator = IgnoreWalkLocator::new(root);
    let result = Workspace::load(root.to_path_buf(), &locator, &runner);

    match result {
        Err(callisto_graph::error::GraphError::ConflictingGroupMembership { groups, .. }) => {
            let names: Vec<String> = groups.iter().map(|g| g.as_str().to_string()).collect();
            assert!(names.contains(&"fixed-a".to_string()));
            assert!(names.contains(&"linked-b".to_string()));
        }
        Err(other) => {
            panic!("expected Err(ConflictingGroupMembership) naming fixed-a and linked-b, got Err({other:?})")
        }
        Ok(_) => panic!("expected Err(ConflictingGroupMembership) naming fixed-a and linked-b, got Ok(_)"),
    }
}

/// A workspace where each resolved PackageId belongs to exactly
/// one group (the ordinary, allowed case) must resolve Ok, and
/// GraphError::ConflictingGroupMembership must never be constructed during
/// that resolve() call.
#[test]
fn non_conflicting_group_membership_resolves_ok() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_two_crate_workspace(root, "alpha", "beta");

    fs::write(
        root.join("callisto.toml"),
        r#"
[[fixed-group]]
name = "fixed-a"
members = ["cargo/alpha"]

[[linked-group]]
name = "linked-b"
members = ["cargo/beta"]
"#,
    )
    .unwrap();

    let runner = NoopRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws =
        Workspace::load(root.to_path_buf(), &locator, &runner).expect("non-conflicting group config must resolve Ok");

    assert!(ws
        .config
        .groups
        .fixed
        .contains_key(&callisto_model::GroupName("fixed-a".to_string())));
    assert!(ws
        .config
        .groups
        .linked
        .contains_key(&callisto_model::GroupName("linked-b".to_string())));
}

/// The colon form is not a spelling of a package: a group member written `cargo:my-lib` fails with the slash form named.
#[test]
fn a_group_member_in_the_colon_form_is_rejected_with_the_slash_form_named() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_two_crate_workspace(root, "my-lib", "other-lib");
    fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"g\"\nmembers = [\"cargo:my-lib\"]\n",
    )
    .unwrap();

    let runner = NoopRunner;
    let locator = IgnoreWalkLocator::new(root);
    let Err(error) = Workspace::load(root.to_path_buf(), &locator, &runner) else {
        panic!("the colon form must be rejected");
    };
    let rendered = format!("{error}");
    assert!(
        rendered.contains("cargo:my-lib") && rendered.contains("colon form"),
        "{rendered}"
    );
    let help = miette::Diagnostic::help(&error)
        .map(|h| h.to_string())
        .unwrap_or_default();
    assert!(help.contains("cargo/my-lib"), "{help}");
    assert_eq!(
        miette::Diagnostic::code(&error).map(|c| c.to_string()).as_deref(),
        Some("E316")
    );
}

/// A group member without its ecosystem fails with the qualified form's shape named.
#[test]
fn a_group_member_without_its_ecosystem_is_rejected() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_two_crate_workspace(root, "my-lib", "other-lib");
    fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"g\"\nmembers = [\"my-lib\"]\n",
    )
    .unwrap();

    let runner = NoopRunner;
    let locator = IgnoreWalkLocator::new(root);
    let Err(error) = Workspace::load(root.to_path_buf(), &locator, &runner) else {
        panic!("a bare group member must be rejected");
    };
    let rendered = format!("{error}");
    assert!(
        rendered.contains("group `g` member") && rendered.contains("`my-lib`"),
        "{rendered}"
    );
    let help = miette::Diagnostic::help(&error)
        .map(|h| h.to_string())
        .unwrap_or_default();
    assert!(help.contains("cargo/my-lib"), "{help}");
    assert_eq!(
        miette::Diagnostic::code(&error).map(|c| c.to_string()).as_deref(),
        Some("E317")
    );
}
