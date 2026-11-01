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

fn execute_with(root: &Path, external: &Path, intent: &Path, extra: &[&str]) -> Output {
    let receipt = external.join("release-receipt.json");
    let mut args = vec![
        "release",
        "execute",
        "--intent",
        intent.to_str().unwrap(),
        "--receipt",
        receipt.to_str().unwrap(),
        "--orchestration-revision",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ];
    args.extend(extra);
    callisto(root, &args)
}

fn assert_execute_refused(external: &Path, out: &Output, code: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "must be refused: {stderr}");
    assert!(stderr.contains(code), "must fail with {code}: {stderr}");
    assert!(!external.join("release-receipt.json").exists());
}

#[test]
fn execute_with_artifact_slots_requires_both_manifest_and_directory() {
    let product = product();
    let external = product.external.path();
    assert!(manifest(&product, &product.artifacts).status.success());
    let sealed = external.join("artifact-manifest.json");

    let neither = execute_with(&product.root, external, &product.intent, &[]);
    assert_execute_refused(external, &neither, "E233");
    let only_manifest = execute_with(
        &product.root,
        external,
        &product.intent,
        &["--artifact-manifest", sealed.to_str().unwrap()],
    );
    assert_execute_refused(external, &only_manifest, "E233");
}

#[test]
fn execute_without_artifact_slots_rejects_artifact_inputs() {
    let product = product();
    assert!(manifest(&product, &product.artifacts).status.success());
    let sealed = product.external.path().join("artifact-manifest.json");
    let (repo, release_commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let intent = plan_intent(repo.path(), external.path(), &release_commit);
    let dir = external.path().to_str().unwrap();

    let only_manifest = execute_with(
        repo.path(),
        external.path(),
        &intent,
        &["--artifact-manifest", sealed.to_str().unwrap()],
    );
    assert_execute_refused(external.path(), &only_manifest, "E232");
    let both = execute_with(
        repo.path(),
        external.path(),
        &intent,
        &["--artifact-manifest", sealed.to_str().unwrap(), "--artifact-dir", dir],
    );
    assert_execute_refused(external.path(), &both, "E232");
}

#[test]
fn plan_for_a_product_release_requires_both_orchestration_flags() {
    let (repo, release_commit) = product_release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let out_path = external.path().join("release-intent.json");
    for flags in [&["--orchestration-revision", release_commit.as_str()][..], &[][..]] {
        let mut args = vec![
            "release",
            "plan",
            "--from-release-commit",
            release_commit.as_str(),
            "--decision",
            DECISION_PATH,
            "--out",
            out_path.to_str().unwrap(),
        ];
        args.extend(flags);
        let out = callisto(repo.path(), &args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{flags:?} must be refused: {stderr}");
        assert!(stderr.contains("E231"), "{flags:?}: {stderr}");
        assert!(!out_path.exists());
    }
}

#[test]
fn plan_without_a_release_section_ignores_the_orchestration_flags_and_plans_no_slots() {
    let (repo, release_commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let out_path = external.path().join("release-intent.json");
    let out = callisto(
        repo.path(),
        &[
            "release",
            "plan",
            "--from-release-commit",
            &release_commit,
            "--decision",
            DECISION_PATH,
            "--orchestration-revision",
            &release_commit,
            "--artifact-repository",
            "example/core-crate",
            "--out",
            out_path.to_str().unwrap(),
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("ignored: the source has no [release] section"),
        "{stderr}"
    );
    let intent: serde_json::Value = serde_json::from_slice(&fs::read(&out_path).unwrap()).unwrap();
    assert_eq!(intent["artifactSlots"].as_array().map(Vec::len), Some(0));
}
