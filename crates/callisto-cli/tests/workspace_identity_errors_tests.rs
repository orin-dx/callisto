//! WS-ID-08 (E101) and WS-ID-09 (E007): two identity errors that behaved correctly but had no test.
#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;

use release_harness::*;

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    callisto_fixtures::git::init_repo(dir.path());
    dir
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn write(root: &std::path::Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

// WS-ID-08: a package's explicit `tag-template` that equals another package's default tags is E101.
#[test]
fn an_explicit_tag_template_equal_to_another_packages_default_fails_with_e101() {
    let dir = repo();
    let root = dir.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    );
    for name in ["a", "b"] {
        write(
            root,
            &format!("crates/{name}/Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
        write(root, &format!("crates/{name}/src/lib.rs"), "");
    }
    write(
        root,
        "callisto.toml",
        "[[package]]\nmatch = \"cargo/a\"\ntag-template = \"b@{version}\"\n",
    );
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "workspace"]);

    let out = callisto(root, &["status"]);
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("E101"), "stderr: {}", stderr(&out));
    assert!(stderr(&out).contains("b@{version}"), "stderr: {}", stderr(&out));
}

// WS-ID-09: co-located Cargo.toml and pyproject.toml disagree on version grammar, static or dynamic.
fn colocated(root: &std::path::Path, python_version: &str) {
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"both\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(root, "src/lib.rs", "");
    write(
        root,
        "pyproject.toml",
        &format!("[project]\nname = \"both\"\n{python_version}\n"),
    );
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "colocated"]);
}

#[test]
fn colocated_cargo_and_python_manifests_fail_with_e007_for_a_static_and_a_dynamic_version() {
    for python_version in ["version = \"0.1.0\"", "dynamic = [\"version\"]"] {
        let dir = repo();
        let root = dir.path();
        colocated(root, python_version);
        write(root, ".changeset/c.md", "---\nboth: patch\n---\n\nFix.\n");

        let before = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        let version = callisto(root, &["version"]);
        assert_eq!(version.status.code(), Some(1), "{python_version}: {}", stderr(&version));
        assert!(
            stderr(&version).contains("E007"),
            "{python_version}: {}",
            stderr(&version)
        );
        assert_eq!(
            fs::read_to_string(root.join("Cargo.toml")).unwrap(),
            before,
            "version must fail before writing anything"
        );

        let init = callisto(root, &["init", "--yes"]);
        assert!(stderr(&init).contains("E007"), "{python_version}: {}", stderr(&init));
        assert!(!root.join("callisto.toml").exists(), "init must fail before writing");
    }
}
