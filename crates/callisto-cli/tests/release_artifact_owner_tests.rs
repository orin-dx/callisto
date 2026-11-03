//! An artifact slot names the package that built it, which need not be the product.
#[path = "common/release_harness.rs"]
mod release_harness;

use std::{fs, path::Path};

use release_harness::*;
use tempfile::TempDir;

/// Product `core` plus a `plugin` crate that ships `plugin.wasm` on the product's
/// release. Only `core` has a changeset; `plugin` is released only via the group.
fn plugin_artifact_fixture(plugin_in_fixed_group: bool) -> (TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    git(
        root,
        &["remote", "add", "origin", "https://github.com/example/core-crate.git"],
    );
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/core\", \"crates/plugin\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    for (dir_name, name) in [("core", "core-crate"), ("plugin", "plugin-crate")] {
        fs::create_dir_all(root.join(format!("crates/{dir_name}/src"))).unwrap();
        fs::write(
            root.join(format!("crates/{dir_name}/Cargo.toml")),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        fs::write(root.join(format!("crates/{dir_name}/src/lib.rs")), "").unwrap();
    }
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial workspace"]);
    git(root, &["tag", "core-crate@0.1.0"]);
    git(root, &["tag", "plugin-crate@0.1.0"]);

    let init = callisto(root, &["init", "--yes", "--versioning", "independent"]);
    assert!(init.status.success(), "init: {}", String::from_utf8_lossy(&init.stderr));
    let config = fs::read_to_string(root.join("callisto.toml")).unwrap();
    let group = if plugin_in_fixed_group {
        "\n[[fixed-group]]\nname = \"product\"\nmembers = [\"cargo/core-crate\", \"cargo/plugin-crate\"]\n"
    } else {
        ""
    };
    fs::write(
        root.join("callisto.toml"),
        format!(
            "{config}\n[release]\nproduct-package = \"cargo/core-crate\"\nforge-repository = \"example/core-crate\"\n\n\
             [[release.artifact]]\npackage = \"cargo/core-crate\"\ntarget = \"x86_64-unknown-linux-gnu\"\nasset-name = \"core-linux.tar.gz\"\n\n\
             [[release.artifact]]\npackage = \"cargo/plugin-crate\"\ntarget = \"wasm32-wasip1\"\nasset-name = \"plugin.wasm\"\n\n\
             [[package]]\nmatch = \"cargo/core-crate\"\npublish-to = [\"crates-io\", \"github-release\"]\n{group}"
        ),
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "configure callisto"]);

    let add = callisto(
        root,
        &["add", "--package", "cargo/core-crate:minor", "--summary", "Ship"],
    );
    assert!(add.status.success(), "add: {}", String::from_utf8_lossy(&add.stderr));
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "add changeset"]);
    let version = callisto(root, &["version", "--emit-decision", DECISION_PATH]);
    assert!(
        version.status.success(),
        "version: {}",
        String::from_utf8_lossy(&version.stderr)
    );
    for entry in fs::read_dir(root.join(".changeset")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|n| n != "README.md") && path.extension().is_some_and(|e| e == "md") {
            fs::remove_file(path).unwrap();
        }
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "release"]);
    let release_commit = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "--detach", &release_commit]);
    (dir, release_commit)
}

fn plan(root: &Path, out: &Path, commit: &str) -> std::process::Output {
    callisto(
        root,
        &[
            "release",
            "plan",
            "--from-release-commit",
            commit,
            "--decision",
            DECISION_PATH,
            "--orchestration-revision",
            commit,
            "--artifact-repository",
            "example/core-crate",
            "--out",
            out.to_str().unwrap(),
        ],
    )
}

#[test]
fn a_plugin_artifact_is_owned_by_the_package_that_builds_it() {
    let (dir, commit) = plugin_artifact_fixture(true);
    let external = tempfile::tempdir().unwrap();
    let intent_path = external.path().join("intent.json");
    let out = plan(dir.path(), &intent_path, &commit);
    assert!(out.status.success(), "plan: {}", String::from_utf8_lossy(&out.stderr));

    let intent: serde_json::Value = serde_json::from_slice(&fs::read(&intent_path).unwrap()).unwrap();
    let owner_of = |asset: &str| {
        intent["artifactSlots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|slot| slot["assetName"] == asset)
            .unwrap_or_else(|| panic!("no slot for {asset}: {intent}"))["package"]
            .to_string()
    };
    assert!(
        owner_of("plugin.wasm").contains("plugin-crate"),
        "{}",
        owner_of("plugin.wasm")
    );
    assert!(
        owner_of("core-linux.tar.gz").contains("core-crate"),
        "{}",
        owner_of("core-linux.tar.gz")
    );
}

#[test]
fn an_artifact_whose_package_is_not_released_is_a_config_error_not_a_defect() {
    let (dir, commit) = plugin_artifact_fixture(false);
    let external = tempfile::tempdir().unwrap();
    let out = plan(dir.path(), &external.path().join("intent.json"), &commit);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "plan must fail");
    assert!(stderr.contains("E179"), "expected E179, got: {stderr}");
    assert!(
        !stderr.contains("E171"),
        "must not be reported as an internal defect: {stderr}"
    );
}

// REL-CMD-04 and REL-CMD-05: a preview needs no `origin`, even when the release builds artifacts.
#[test]
fn a_preview_without_origin_plans_an_artifact_release() {
    let (dir, _) = plugin_artifact_fixture(true);
    git(dir.path(), &["remote", "remove", "origin"]);
    let out = callisto(dir.path(), &["release", "--dry-run"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "preview must succeed without origin: {stderr}");
    let plan: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(plan["dryRun"], true, "{plan}");
    assert!(plan["digest"].is_string(), "{plan}");
    assert!(!stderr.contains("E170"), "stderr: {stderr}");

    let run = callisto(dir.path(), &["release"]);
    assert!(!run.status.success(), "the real run still needs origin");
}

// REL-AUTH-08: `--artifact-repository` alone is not a usage error; a source with `[release]` fails with E231.
#[test]
fn plan_with_only_the_artifact_repository_fails_with_e231_not_a_usage_error() {
    let (dir, commit) = plugin_artifact_fixture(true);
    let external = tempfile::tempdir().unwrap();
    let out = callisto(
        dir.path(),
        &[
            "release",
            "plan",
            "--from-release-commit",
            &commit,
            "--decision",
            DECISION_PATH,
            "--artifact-repository",
            "example/core-crate",
            "--out",
            external.path().join("intent.json").to_str().unwrap(),
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert!(stderr.contains("E231"), "expected E231, got: {stderr}");
}

// REL-AUTH-05: every route fails with E266 for a shallow repository, not a lower-level read failure.
#[test]
fn plan_from_a_release_commit_in_a_shallow_clone_fails_with_e266() {
    let (dir, commit) = plugin_artifact_fixture(true);
    let shallow = tempfile::tempdir().unwrap();
    git(shallow.path(), &["init", "-q"]);
    git(
        shallow.path(),
        &["fetch", "-q", "--depth", "1", dir.path().to_str().unwrap(), &commit],
    );
    git(shallow.path(), &["checkout", "-q", "--detach", "FETCH_HEAD"]);
    let external = tempfile::tempdir().unwrap();
    let out = plan(shallow.path(), &external.path().join("intent.json"), &commit);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "plan must fail");
    assert!(stderr.contains("E266"), "expected E266, got: {stderr}");
}

// REL-AUTH-05: and with E265 for a repository that is not SHA-1.
#[test]
fn plan_from_a_release_commit_in_a_sha256_repository_fails_with_e265() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "--object-format=sha256"]);
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"p\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    let external = tempfile::tempdir().unwrap();
    let out = plan(
        dir.path(),
        &external.path().join("intent.json"),
        "0123456789abcdef0123456789abcdef01234567",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "plan must fail");
    assert!(stderr.contains("E265"), "expected E265, got: {stderr}");
}
