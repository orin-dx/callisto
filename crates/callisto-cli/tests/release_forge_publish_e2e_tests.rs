#![cfg(unix)]

//! Black-box coverage of the forge publication boundary.
//!
//! A GitHub Release is created as a draft, every product asset is uploaded to
//! that draft, and only then is the release published. These tests drive the
//! compiled binary against the strict fake `gh`, which models GitHub's real
//! behaviour: the tag endpoint does not serve drafts, the list endpoint does,
//! and `release edit --draft=false` is what publishes.

#[path = "common/release_harness.rs"]
mod release_harness;

use std::fs;
use std::path::Path;

use release_harness::*;

const FORGE_TAG: &str = "callisto@0.2.0";

fn artifact_manifest(root: &Path, intent: &Path, artifacts: &Path, external: &Path) -> std::path::PathBuf {
    let manifest = external.join("artifact-manifest.json");
    let created = callisto(
        root,
        &[
            "release",
            "artifact-manifest",
            "--intent",
            intent.to_str().unwrap(),
            "--artifact-dir",
            artifacts.to_str().unwrap(),
            "--out",
            manifest.to_str().unwrap(),
        ],
    );
    assert!(
        created.status.success(),
        "artifact manifest creation failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    manifest
}

/// The product fixture plus its manifest, artifacts, and a strict rig.
struct ProductRun {
    _dir: tempfile::TempDir,
    _external: tempfile::TempDir,
    root: std::path::PathBuf,
    intent: std::path::PathBuf,
    manifest: std::path::PathBuf,
    artifacts: std::path::PathBuf,
    receipt: std::path::PathBuf,
    rig: Rig,
}

impl ProductRun {
    fn new() -> Self {
        let (dir, release_commit) = product_release_commit_fixture();
        let external = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let intent = plan_product_intent(&root, external.path(), &release_commit);
        let artifacts = create_product_artifacts(external.path());
        let manifest = artifact_manifest(&root, &intent, &artifacts, external.path());
        let rig = Rig::new(external.path(), &release_commit);
        Self {
            receipt: external.path().join("release-receipt.json"),
            root,
            intent,
            manifest,
            artifacts,
            rig,
            _dir: dir,
            _external: external,
        }
    }

    fn execute(&self, extra: &[&str]) -> std::process::Output {
        let mut args = vec![
            "--artifact-manifest",
            self.manifest.to_str().unwrap(),
            "--artifact-dir",
            self.artifacts.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        execute_rig(
            &self.root,
            &self.intent,
            &self.receipt,
            &self.rig,
            FORGE_TAG,
            &args,
            None,
        )
    }

    /// Every mutating `gh release` invocation, in the order it was issued.
    fn release_effects(&self) -> Vec<String> {
        self.rig
            .gh_calls()
            .into_iter()
            .filter(|call| call.starts_with("release "))
            .collect()
    }

    fn assert_argv_allowed(&self) {
        assert_argv_within_allowlists(&self.rig.log, &self.rig.git_trace);
    }

    fn drop_receipt(&self) {
        fs::remove_file(&self.receipt).unwrap();
    }
}

fn index_of(effects: &[String], prefix: &str) -> usize {
    effects
        .iter()
        .position(|call| call.starts_with(prefix))
        .unwrap_or_else(|| panic!("expected a `gh {prefix}` effect in {effects:?}"))
}

#[test]
fn a_release_is_drafted_then_filled_then_published_in_that_order() {
    let run = ProductRun::new();
    let output = run.execute(&[]);
    assert!(
        output.status.success(),
        "product release execution failed: {}\n{:?}",
        stderr_of(&output),
        run.rig.gh_calls()
    );

    let effects = run.release_effects();
    assert_eq!(
        effects.iter().filter(|call| call.starts_with("release create")).count(),
        1
    );
    assert!(
        effects[0].contains("--draft"),
        "the release must be created as a draft: {effects:?}"
    );
    assert!(
        !effects[0].contains("--prerelease"),
        "0.2.0 has no semver pre-release part: {effects:?}"
    );

    let created = index_of(&effects, "release create");
    let published = index_of(&effects, "release edit");
    assert!(
        effects[published].contains("--draft=false"),
        "publication is `release edit --draft=false`: {effects:?}"
    );
    let uploads: Vec<usize> = effects
        .iter()
        .enumerate()
        .filter(|(_, call)| call.starts_with("release upload"))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(uploads.len(), PRODUCT_ASSETS.len(), "{effects:?}");
    for upload in &uploads {
        assert!(
            created < *upload && *upload < published,
            "every asset must be uploaded after the draft and before publication: {effects:?}"
        );
    }
    assert!(
        !effects.iter().any(|call| call.contains("--clobber")),
        "an upload must never overwrite a differing asset: {effects:?}"
    );
    run.assert_argv_allowed();
}

/// The window this whole change exists to close: if the process dies after the
/// draft exists but before its assets are attached, a rerun must fill the
/// draft and publish it, not leave a published, incomplete release behind.
#[test]
fn a_rerun_over_a_half_filled_draft_uploads_the_rest_and_then_publishes() {
    let run = ProductRun::new();
    // A draft carrying no assets: exactly what an interrupted run leaves.
    fs::write(&run.rig.forge_marker, "draft").unwrap();

    let output = run.execute(&[]);
    assert!(
        output.status.success(),
        "a rerun over an existing draft failed: {}\n{:?}",
        stderr_of(&output),
        run.rig.gh_calls()
    );
    let effects = run.release_effects();
    assert!(
        !effects.iter().any(|call| call.starts_with("release create")),
        "an observed draft must be adopted, never recreated: {effects:?}"
    );
    assert_eq!(
        effects.iter().filter(|call| call.starts_with("release upload")).count(),
        PRODUCT_ASSETS.len(),
        "{effects:?}"
    );
    assert!(
        index_of(&effects, "release edit") > index_of(&effects, "release upload"),
        "publication still comes last on a rerun: {effects:?}"
    );
    run.assert_argv_allowed();
}

#[test]
fn a_release_already_published_with_every_asset_is_already_satisfied() {
    let run = ProductRun::new();
    let first = run.execute(&[]);
    assert!(first.status.success(), "{}", stderr_of(&first));
    let before = run.release_effects();

    run.drop_receipt();
    let second = run.execute(&[]);
    assert!(
        second.status.success(),
        "a rerun over a complete release failed: {}",
        stderr_of(&second)
    );
    assert_eq!(
        run.release_effects(),
        before,
        "a complete release must be adopted from observation, with no effect re-issued"
    );
    assert!(run.receipt.exists());
}

/// A published release missing an asset is an absent upload, not a conflict:
/// the asset's own identity is what the operation is about, and `Absent` is
/// the only observation that authorizes an effect. GitHub will refuse the
/// upload if the release is immutable, and that refusal surfaces as a typed
/// command failure rather than a release wrongly reported complete.
#[test]
fn a_published_release_missing_an_asset_re_uploads_only_that_asset() {
    let run = ProductRun::new();
    let first = run.execute(&[]);
    assert!(first.status.success(), "{}", stderr_of(&first));

    let marker = run.rig.log.with_extension("artifact-marker");
    let assets = fs::read_to_string(&marker).unwrap();
    let kept: Vec<&str> = assets.lines().skip(1).collect();
    let dropped = assets.lines().next().unwrap().to_owned();
    fs::write(&marker, format!("{}\n", kept.join("\n"))).unwrap();

    run.drop_receipt();
    let before = run.release_effects().len();
    let recovered = run.execute(&[]);
    assert!(
        recovered.status.success(),
        "a rerun over a missing asset failed: {}",
        stderr_of(&recovered)
    );
    let after: Vec<String> = run.release_effects().into_iter().skip(before).collect();
    let name = dropped.split('|').next().unwrap();
    assert_eq!(
        after.len(),
        1,
        "only the missing asset's upload may be re-issued: {after:?}"
    );
    assert!(
        after[0].starts_with("release upload") && after[0].contains(name),
        "{after:?}"
    );
}

/// The one gap the tag endpoint cannot see: a draft is only visible in the
/// list endpoint, and it may not be on the first page.
#[test]
fn a_draft_listed_on_the_second_page_is_found_and_never_recreated() {
    let mut run = ProductRun::new();
    fs::write(&run.rig.forge_marker, "draft").unwrap();
    run.rig.set("CALLISTO_TEST_FORGE_PAGE", "2");

    let output = run.execute(&[]);
    assert!(
        output.status.success(),
        "a draft on page 2 must still be observed: {}\n{:?}",
        stderr_of(&output),
        run.rig.gh_calls()
    );
    let effects = run.release_effects();
    assert!(
        !effects.iter().any(|call| call.starts_with("release create")),
        "{effects:?}"
    );
    assert!(effects.iter().any(|call| call.contains("--draft=false")), "{effects:?}");
}

/// The release is not a pre-release, so a remote release flagged as one is a
/// different object than this intent authorized.
#[test]
fn a_prerelease_flag_mismatch_is_a_conflict_and_issues_no_effect() {
    let run = ProductRun::new();
    fs::write(&run.rig.forge_marker, "published").unwrap();
    fs::write(run.rig.forge_marker.with_extension("prerelease"), "").unwrap();

    let output = run.execute(&[]);
    assert!(
        !output.status.success(),
        "a disagreeing prerelease flag must stop the release: {:?}",
        run.rig.gh_calls()
    );
    let effects = run.release_effects();
    assert!(
        effects.is_empty(),
        "a conflicting forge release must be observed, not edited: {effects:?}"
    );
    assert!(
        diagnostic_codes(&output).iter().any(|code| code == "E167"),
        "the failure must be the typed remote-conflict, not an unrelated error: {}",
        stderr_of(&output)
    );
}

/// A release with no product artifacts has nothing to wait for, so its
/// publication depends on the draft alone -- and still happens.
#[test]
fn a_release_with_no_artifacts_is_still_published() {
    let (dir, release_commit) = release_commit_fixture();
    let external = tempfile::tempdir().unwrap();
    let root = dir.path();
    let intent = plan_intent(root, external.path(), &release_commit);
    let receipt = external.path().join("release-receipt.json");
    let rig = Rig::new(external.path(), &release_commit);

    let output = execute_rig(root, &intent, &receipt, &rig, "core-crate@0.2.0", &[], None);
    assert!(
        output.status.success(),
        "zero-artifact release execution failed: {}\n{:?}",
        stderr_of(&output),
        rig.gh_calls()
    );
    let effects: Vec<String> = rig
        .gh_calls()
        .into_iter()
        .filter(|call| call.starts_with("release "))
        .collect();
    assert!(
        effects[0].contains("release create") && effects[0].contains("--draft"),
        "{effects:?}"
    );
    assert!(
        !effects.iter().any(|call| call.starts_with("release upload")),
        "{effects:?}"
    );
    assert!(
        effects.last().is_some_and(|call| call.contains("--draft=false")),
        "publication must still close a release that carries no assets: {effects:?}"
    );
}
