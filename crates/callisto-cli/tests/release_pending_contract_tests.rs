//! The release workflow reads `callisto --format json release pending` with jq (`.pending[0].source`).
//! A renamed key would read as "nothing pending" and skip a stuck release, so the real output is pinned here.
#[path = "common/release_harness.rs"]
mod release_harness;

use release_harness::*;

#[test]
fn release_pending_json_carries_the_keys_the_workflow_reads() {
    let (dir, release_commit) = release_commit_fixture();
    let out = callisto(dir.path(), &["release", "pending"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();

    assert_eq!(report["schemaVersion"], 1, "{report}");
    assert_eq!(report["command"], "release pending", "{report}");
    assert_eq!(report["dryRun"], false, "{report}");

    // `.pending[0].source` is what callisto-release.yml selects as the release to finish.
    assert_eq!(report["pending"][0]["source"], release_commit, "{report}");
    assert_eq!(
        report["pending"][0]["packages"][0]["package"], "cargo/core-crate",
        "{report}"
    );
    assert_eq!(report["pending"][0]["packages"][0]["version"], "0.2.0", "{report}");
    let missing = report["pending"][0]["missing"].as_array().expect("missing is an array");
    assert!(
        missing
            .iter()
            .any(|gap| gap.as_str().is_some_and(|gap| gap.contains("core-crate@0.2.0"))),
        "{report}"
    );
}

#[test]
fn release_pending_json_is_an_empty_list_when_every_release_finished() {
    let (dir, _) = release_commit_fixture();
    git(dir.path(), &["tag", "-a", "-m", "release", "core-crate@0.2.0"]);
    let out = callisto(dir.path(), &["release", "pending"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["pending"], serde_json::json!([]), "{report}");
    assert_eq!(report["schemaVersion"], 1, "{report}");
    assert_eq!(report["command"], "release pending", "{report}");
}
