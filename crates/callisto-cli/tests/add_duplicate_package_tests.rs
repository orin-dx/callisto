//! `callisto add --package` rejects a package named twice instead of writing a changeset Callisto cannot read back.

use std::path::Path;
use std::process::{Command, Output, Stdio};

fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"a\", \"b\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    for name in ["a", "b"] {
        std::fs::create_dir_all(root.join(format!("{name}/src"))).unwrap();
        std::fs::write(
            root.join(format!("{name}/Cargo.toml")),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        std::fs::write(root.join(format!("{name}/src/lib.rs")), "").unwrap();
    }
    std::fs::write(root.join("callisto.toml"), "").unwrap();
    dir
}

fn add(root: &Path, packages: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_callisto"));
    cmd.args(["--cwd", root.to_str().unwrap(), "add", "--summary", "s"]);
    for package in packages {
        cmd.args(["--package", package]);
    }
    cmd.stdin(Stdio::null()).output().unwrap()
}

fn changesets(root: &Path) -> Vec<String> {
    std::fs::read_dir(root.join(".changeset"))
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|name| name.ends_with(".md") && name != "README.md")
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_package_given_twice_is_rejected_and_nothing_is_written() {
    let dir = workspace();
    let root = dir.path();
    for packages in [["cargo/a:patch", "cargo/a:patch"], ["cargo/a:patch", "cargo/a:major"]] {
        let out = add(root, &packages);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{packages:?}: {stderr}");
        assert!(
            stderr.contains("E329") && stderr.contains("cargo/a"),
            "{packages:?}: {stderr}"
        );
        assert!(changesets(root).is_empty(), "{packages:?} must write nothing");
    }
}

#[test]
fn distinct_packages_still_write_one_changeset() {
    let dir = workspace();
    let root = dir.path();
    let out = add(root, &["cargo/a:patch", "cargo/b:major"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let files = changesets(root);
    assert_eq!(files.len(), 1);
    let text = std::fs::read_to_string(root.join(".changeset").join(&files[0])).unwrap();
    assert!(text.contains("a: patch") && text.contains("b: major"), "{text}");
}
