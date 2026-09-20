//! Durable release does not support PyPI: pip cannot distinguish a missing
//! project from an unreachable index, so a PyPI version can never be proved
//! absent and no PyPI publish may be planned.

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
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Callisto Test"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["config", "tag.gpgsign", "false"]);
    git(
        root,
        &["remote", "add", "origin", "https://github.com/example/demo.git"],
    );
    fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\nresolver = \"2\"\n").unwrap();
    fs::create_dir_all(root.join(manifest_path).parent().unwrap()).unwrap();
    fs::write(root.join(manifest_path), manifest).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial"]);
    let init = callisto(root, &["init", "--yes"]);
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

/// `release plan` must refuse, name the reason, and leave no intent behind.
fn plan_is_refused(dir: &tempfile::TempDir, package: &str) {
    let external = tempfile::tempdir().unwrap();
    let out = external.path().join("intent.json");
    let plan = callisto(
        dir.path(),
        &["release", "plan", "--package", package, "--out", out.to_str().unwrap()],
    );
    let stdout = String::from_utf8_lossy(&plan.stdout);
    let stderr = String::from_utf8_lossy(&plan.stderr);
    let printed = format!("{stdout}{stderr}");
    assert!(
        !plan.status.success(),
        "release plan succeeded for {package}, which durable release cannot observe\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(printed.contains("E170"), "the refusal must be typed: {printed}");
    assert!(
        printed.contains("pip cannot distinguish") && printed.contains("unreachable index"),
        "the refusal must say why PyPI cannot be observed: {printed}"
    );
    assert!(
        printed.contains("publish it outside durable release"),
        "the refusal must say how to proceed: {printed}"
    );
    assert!(!out.exists(), "a refused plan must not leave an intent file behind");
}

#[test]
fn a_pypi_publish_target_is_refused_at_plan_time_because_pip_cannot_prove_absence() {
    let dir = fixture(
        "pkg/pyproject.toml",
        "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        "pypi/demo",
        "pypi",
    );
    plan_is_refused(&dir, "pypi/demo");
}
