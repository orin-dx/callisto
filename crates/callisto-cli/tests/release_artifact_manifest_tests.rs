#![cfg(unix)]

//! Black-box tests that `release artifact-manifest` only digests the exact regular files an
//! intent declares: anything else in the artifact directory is refused or ignored, never
//! followed, and a failed run leaves no manifest behind.

#[path = "common/release_harness.rs"]
mod release_harness;

use std::{fs, os::unix::fs::symlink, path::Path, process::Output};

use release_harness::*;

const ASSET: &str = "callisto-moon.wasm";

struct Product {
    _repo: tempfile::TempDir,
    external: tempfile::TempDir,
    root: std::path::PathBuf,
    intent: std::path::PathBuf,
    artifacts: std::path::PathBuf,
}

fn product() -> Product {
    let (repo, release_commit) = product_release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let root = repo.path().to_path_buf();
    let intent = plan_product_intent(&root, external.path(), &release_commit);
    let artifacts = create_product_artifacts(external.path());
    Product {
        _repo: repo,
        external,
        root,
        intent,
        artifacts,
    }
}

fn manifest(product: &Product, artifact_dir: &Path) -> Output {
    callisto(
        &product.root,
        &[
            "release",
            "artifact-manifest",
            "--intent",
            product.intent.to_str().unwrap(),
            "--artifact-dir",
            artifact_dir.to_str().unwrap(),
            "--out",
            product.external.path().join("artifact-manifest.json").to_str().unwrap(),
        ],
    )
}

fn assert_refused(product: &Product, out: &Output, code: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "must be refused: {stderr}");
    assert!(stderr.contains(code), "must fail with {code}: {stderr}");
    assert!(
        !product.external.path().join("artifact-manifest.json").exists(),
        "a refused run must not write a manifest"
    );
}

#[test]
fn a_manifest_covers_exactly_the_declared_assets_and_ignores_everything_else() {
    let product = product();
    let out = manifest(&product, &product.artifacts);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(product.external.path().join("artifact-manifest.json")).unwrap()).unwrap();
    let entries = written["entries"].as_array().unwrap();
    assert_eq!(entries.len(), PRODUCT_ASSETS.len());
    let text = serde_json::to_string(&written).unwrap();
    assert!(!text.contains("hidden-metadata") && !text.contains("unlisted-artifact"));
}

#[test]
fn a_symlinked_asset_is_refused_even_when_it_points_at_a_real_file() {
    let product = product();
    let real = product.external.path().join("real-asset");
    fs::write(&real, "real bytes\n").unwrap();
    fs::remove_file(product.artifacts.join(ASSET)).unwrap();
    symlink(&real, product.artifacts.join(ASSET)).unwrap();
    assert_refused(&product, &manifest(&product, &product.artifacts), "E221");
}

#[test]
fn a_directory_in_place_of_an_asset_is_refused() {
    let product = product();
    fs::remove_file(product.artifacts.join(ASSET)).unwrap();
    fs::create_dir(product.artifacts.join(ASSET)).unwrap();
    assert_refused(&product, &manifest(&product, &product.artifacts), "E221");
}

#[test]
fn a_missing_asset_is_refused() {
    let product = product();
    fs::remove_file(product.artifacts.join(ASSET)).unwrap();
    let out = manifest(&product, &product.artifacts);
    assert!(!out.status.success());
    assert!(!product.external.path().join("artifact-manifest.json").exists());
}

#[test]
fn an_artifact_directory_that_is_itself_a_symlink_resolves_to_its_real_files() {
    let product = product();
    let link = product.external.path().join("artifacts-link");
    symlink(&product.artifacts, &link).unwrap();
    let out = manifest(&product, &link);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn an_intent_without_artifact_slots_cannot_produce_a_manifest() {
    let (repo, release_commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let intent = plan_intent(repo.path(), external.path(), &release_commit);
    let out = callisto(
        repo.path(),
        &[
            "release",
            "artifact-manifest",
            "--intent",
            intent.to_str().unwrap(),
            "--artifact-dir",
            external.path().to_str().unwrap(),
            "--out",
            external.path().join("artifact-manifest.json").to_str().unwrap(),
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("E219"), "{stderr}");
    assert!(!external.path().join("artifact-manifest.json").exists());
}
