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
    git(root, &["init", "-b", "main"]);
    for (key, value) in [
        ("user.name", "Callisto Test"),
        ("user.email", "test@example.invalid"),
        ("commit.gpgsign", "false"),
        ("tag.gpgsign", "false"),
    ] {
        git(root, &["config", key, value]);
    }
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

    let init = callisto(root, &["init", "--yes"]);
    assert!(init.status.success(), "init: {}", String::from_utf8_lossy(&init.stderr));
    let config = fs::read_to_string(root.join("callisto.toml")).unwrap();
    let group = if plugin_in_fixed_group {
        "\n[[fixed-group]]\nname = \"product\"\nmembers = [\"core-crate\", \"plugin-crate\"]\n"
    } else {
        ""
    };
    fs::write(
        root.join("callisto.toml"),
        format!(
            "{config}\n[release]\nproduct-package = \"cargo/core-crate\"\n\n\
             [[release.artifact]]\npackage = \"cargo/core-crate\"\ntarget = \"x86_64-unknown-linux-gnu\"\nasset-name = \"core-linux.tar.gz\"\n\n\
             [[release.artifact]]\npackage = \"cargo/plugin-crate\"\ntarget = \"wasm32-wasip1\"\nasset-name = \"plugin.wasm\"\n\n\
             [release.profiles.production]\nforge-repository = \"example/core-crate\"\nregistry-routes = {{ cratesIo = \"cratesIo\" }}\n\n\
             [[package]]\nmatch = \"cargo/core-crate\"\npublish-to = [\"crates-io\", \"github-release\"]\n{group}"
        ),
    )
    .unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "configure callisto"]);

    let add = callisto(root, &["add", "--package", "core-crate:minor", "--summary", "Ship"]);
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
