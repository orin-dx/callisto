#![cfg(unix)]

//! A fixed group aligns its members after the cascade first visits them, so dependency rewrites and range rejections
//! must follow the aligned versions, never the provisional per-member bumps.

#[path = "common/release_harness.rs"]
mod release_harness;

use std::{fs, path::Path};

use release_harness::*;

#[derive(Clone, Copy)]
enum Eco {
    Cargo,
    Npm,
    Python,
}

impl Eco {
    fn prefix(self) -> &'static str {
        match self {
            Eco::Cargo => "cargo",
            Eco::Npm => "npm",
            Eco::Python => "pypi",
        }
    }

    /// The manifest of `app`, which depends on `lib-b` through `range`.
    fn app_manifest(self) -> &'static str {
        match self {
            Eco::Cargo => "crates/app/Cargo.toml",
            Eco::Npm => "packages/app/package.json",
            Eco::Python => "packages/app/pyproject.toml",
        }
    }
}

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// `lib-a` (released at 1.0.0) and `lib-b` (never released, at 2.0.0) share a fixed group; `app` requires `lib-b`
/// through `range`. A patch for `lib-b` is provisionally 2.0.1 and aligns the group to 1.0.1.
fn workspace(eco: Eco, range: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    match eco {
        Eco::Cargo => {
            write(
                root,
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
            );
            for (name, version, extra) in [
                ("lib-a", "1.0.0", String::new()),
                ("lib-b", "2.0.0", String::new()),
                (
                    "app",
                    "0.1.0",
                    format!("\n[dependencies]\nlib-b = {{ path = \"../lib-b\", version = \"{range}\" }}\n"),
                ),
            ] {
                write(
                    root,
                    &format!("crates/{name}/Cargo.toml"),
                    &format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n{extra}"),
                );
                write(root, &format!("crates/{name}/src/lib.rs"), "\n");
            }
        }
        Eco::Npm => {
            write(root, "package.json", r#"{"private":true,"workspaces":["packages/*"]}"#);
            for (name, version, deps) in [
                ("lib-a", "1.0.0", String::new()),
                ("lib-b", "2.0.0", String::new()),
                ("app", "0.1.0", format!(r#","dependencies":{{"lib-b":"{range}"}}"#)),
            ] {
                write(
                    root,
                    &format!("packages/{name}/package.json"),
                    &format!(r#"{{"name":"{name}","version":"{version}"{deps}}}"#),
                );
            }
        }
        Eco::Python => {
            write(
                root,
                "pyproject.toml",
                "[tool.uv.workspace]\nmembers = [\"packages/*\"]\n",
            );
            for (name, version, extra) in [
                ("lib-a", "1.0.0", String::new()),
                ("lib-b", "2.0.0", String::new()),
                ("app", "0.1.0", format!("dependencies = [\"lib-b{range}\"]\n")),
            ] {
                write(
                    root,
                    &format!("packages/{name}/pyproject.toml"),
                    &format!("[project]\nname = \"{name}\"\nversion = \"{version}\"\n{extra}"),
                );
            }
        }
    }
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "workspace"]);
    git(root, &["tag", "lib-a@1.0.0"]);
    let init = callisto(root, &["init", "--yes", "--versioning", "independent", "--no-workflow"]);
    assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));
    let config = fs::read_to_string(root.join("callisto.toml")).unwrap();
    let prefix = eco.prefix();
    write(
        root,
        "callisto.toml",
        &format!("{config}\n[[fixed-group]]\nname = \"pair\"\nmembers = [\"{prefix}/lib-a\", \"{prefix}/lib-b\"]\n"),
    );
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "configure callisto"]);
    dir
}

fn patch_lib_b(eco: Eco, root: &Path) -> std::process::Output {
    let package = format!("{}/lib-b:patch", eco.prefix());
    let out = callisto(root, &["add", "--package", &package, "--summary", "change"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "changeset"]);
    callisto(root, &["version"])
}

const ECOSYSTEMS: [Eco; 3] = [Eco::Cargo, Eco::Npm, Eco::Python];

fn persisted(eco: Eco, root: &Path) -> String {
    fs::read_to_string(root.join(eco.app_manifest())).unwrap()
}

/// `range` admits the aligned 1.0.1 and forbids the provisional 2.0.1, so nothing needs rewriting and `app` is not released.
fn assert_untouched(range: &str) {
    for eco in ECOSYSTEMS {
        let dir = workspace(eco, range);
        let before = persisted(eco, dir.path());
        let out = patch_lib_b(eco, dir.path());
        assert!(
            out.status.success(),
            "{}: {}",
            eco.prefix(),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            persisted(eco, dir.path()),
            before,
            "{}: a requirement the aligned version satisfies is left as written and its dependent is not released",
            eco.prefix()
        );
    }
}

#[test]
fn a_requirement_the_aligned_version_satisfies_is_never_widened_to_the_provisional_version() {
    assert_untouched("<=2.0.0");
}

#[test]
fn a_requirement_that_admits_the_aligned_version_is_not_rejected_for_the_provisional_version() {
    assert_untouched("<2.0.1");
}

#[test]
fn a_pin_the_aligned_version_leaves_behind_is_rewritten_to_it() {
    for (eco, pin) in [(Eco::Cargo, "=2.0.0"), (Eco::Npm, "2.0.0"), (Eco::Python, "==2.0.0")] {
        let dir = workspace(eco, pin);
        let out = patch_lib_b(eco, dir.path());
        assert!(
            out.status.success(),
            "{}: {}",
            eco.prefix(),
            String::from_utf8_lossy(&out.stderr)
        );
        let after = persisted(eco, dir.path());
        assert!(after.contains("1.0.1"), "{}: {after}", eco.prefix());
        assert!(
            !after.contains("2.0.1"),
            "{}: the provisional version leaked:\n{after}",
            eco.prefix()
        );
    }
}
