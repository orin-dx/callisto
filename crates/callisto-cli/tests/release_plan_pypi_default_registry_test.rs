//! PyPI's own JSON simple index (PEP 691) answers a version query directly --
//! 200 with the version present or absent, 404 for an unknown project -- so
//! durable release observes it over `curl`, never through `pip`, and `release
//! plan` no longer refuses a PyPI publish target at plan time.

use std::{fs, path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git").args(args).current_dir(root).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn callisto(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--format", "json", "--cwd", root.to_str().unwrap()])
        .args(args)
        .output()
        .unwrap()
}

/// A non-Cargo package published to `registry`, with no `[release]` section.
fn fixture(manifest_path: &str, manifest: &str, package_match: &str, registry: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    git(
        root,
        &["remote", "add", "origin", "https://github.com/example/demo.git"],
    );
    fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
    fs::create_dir_all(root.join(manifest_path).parent().unwrap()).unwrap();
    fs::write(root.join(manifest_path), manifest).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial"]);
    let init = callisto(root, &["init", "--yes", "--versioning", "independent"]);
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    let config = fs::read_to_string(root.join("callisto.toml")).unwrap();
    fs::write(
        root.join("callisto.toml"),
        format!("{config}\n[[package]]\nmatch = \"{package_match}\"\npublish-to = [\"{registry}\"]\n"),
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "configure callisto"]);
    let add = callisto(root, &["add", "--package", "demo:minor", "--summary", "Ship demo"]);
    assert!(
        add.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "add changeset"]);
    let head = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "--detach", &head]);
    dir
}

/// `release plan` derives a registry-observability precondition purely from
/// the ecosystem, with no network call: it must succeed for a PyPI target the
/// same way it does for cargo or npm, and leave a real intent behind.
#[test]
fn a_pypi_publish_target_is_planned_like_any_other_observable_registry() {
    let dir = fixture(
        "pkg/pyproject.toml",
        "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        "pypi/demo",
        "pypi",
    );
    let external = tempfile::tempdir().unwrap();
    let out = external.path().join("intent.json");
    let plan = callisto(
        dir.path(),
        &[
            "release",
            "plan",
            "--package",
            "pypi/demo",
            "--out",
            out.to_str().unwrap(),
        ],
    );
    let stdout = String::from_utf8_lossy(&plan.stdout);
    let stderr = String::from_utf8_lossy(&plan.stderr);
    assert!(
        plan.status.success(),
        "release plan failed for a PyPI target, which durable release can now observe\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(out.exists(), "a successful plan must leave an intent file behind");
    let intent = fs::read_to_string(&out).unwrap();
    assert!(
        intent.contains("pypi/demo"),
        "the intent must name the planned package: {intent}"
    );
}
