//! Regression test for redundant owner-manifest reads in `plan_version`'s
//! platform-target loop (audit finding: perf).
//!
//! Before the fix, `plan_version` (in `commands::version`) opened the
//! OWNER's canonical manifest via `callisto_manifests::open` once per
//! `GroupMember::PlatformManifest` sibling under that owner, purely to check
//! whether the owner already declares a matching optional dependency --
//! re-reading and re-parsing the same file from disk N times for an owner
//! with N platform targets (napi/native cross-compile packages typically
//! have 4-8+). The fix routes that read through the workspace's existing
//! read-only `manifest_cache` (`crate::manifest_cache::open_cached`), so the
//! owner's manifest is opened at most once per run regardless of how many
//! platform siblings it has.
//!
//! Isolated in its own integration-test binary because
//! `callisto_manifests::open_call_count` is a process-global counter that
//! other, non-#[serial] tests in a shared binary would pollute (see
//! `crates/callisto-graph/tests/manifest_cache_test.rs` for the precedent).

use std::path::Path;

use callisto_graph::commands::{plan_version, VersionOptions};
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::{NoInference, Workspace};
use callisto_model::{CommandError, CommandOutput, CommandRunner, GroupName, ManifestRole, PackageId};
use serial_test::serial;

struct NoopRunner;

impl CommandRunner for NoopRunner {
    fn run(&self, _program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
        Ok(CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

fn git_init_with_commit(root: &Path) {
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Test"],
        vec!["config", "commit.gpgsign", "false"],
        vec!["config", "tag.gpgsign", "false"],
    ] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(root)
            .output()
            .expect("git must be installed");
    }
    std::fs::write(root.join(".gitkeep"), "").unwrap();
    for args in [vec!["add", "."], vec!["commit", "-q", "-m", "init"]] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(root)
            .output()
            .expect("git must be installed");
    }
}

/// Builds the same owner-with-two-platform-siblings fixture as
/// `commands::version`'s own `plan_version_merges_two_platform_siblings_optional_dep_updates_into_one_entry`
/// (AC-007b) test: a real, disk-discovered Case D linux sibling
/// (`Cargo.toml` + `package.json` sharing one directory) plus a
/// fixture-injected darwin sibling, both under owner "hybrid", whose
/// `Cargo.toml` declares matching `optional = true` dependencies on both
/// platform names.
fn build_owner_with_two_platform_siblings(root: &Path) {
    std::fs::create_dir_all(root.join("crates/hybrid")).unwrap();
    std::fs::write(
        root.join("crates/hybrid/Cargo.toml"),
        "[package]\nname = \"hybrid\"\nversion = \"1.0.0\"\n\n[dependencies]\n\"@myorg/hybrid-linux-x64-gnu\" = { version = \"0.9.0\", optional = true }\n\"@myorg/hybrid-darwin-arm64\" = { version = \"0.5.0\", optional = true }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("crates/hybrid/package.json"),
        r#"{"name":"@myorg/hybrid-linux-x64-gnu","version":"0.9.0","os":["linux"],"cpu":["x64"]}"#,
    )
    .unwrap();

    std::fs::create_dir_all(root.join("crates/hybrid-darwin")).unwrap();
    std::fs::write(
        root.join("crates/hybrid-darwin/package.json"),
        r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.5.0","os":["darwin"],"cpu":["arm64"]}"#,
    )
    .unwrap();

    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"hybrid\", \"@myorg/hybrid-linux-x64-gnu\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(root.join(".changeset/bump.md"), "---\n\"hybrid\": patch\n---\n\nfix.\n").unwrap();
}

#[test]
#[serial]
fn plan_version_opens_owner_manifest_once_regardless_of_platform_sibling_count() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);
    build_owner_with_two_platform_siblings(root);
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .output()
        .expect("git add");
    std::process::Command::new("git")
        .args(["commit", "-q", "-m", "add hybrid package with platform siblings"])
        .current_dir(root)
        .output()
        .expect("git commit");

    let locator = IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let mut ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    // Fixture-inject the second (darwin) platform sibling -- walk.rs's Case D
    // registration groups strictly by directory, so a second platform
    // manifest under the same owner can't be disk-discovered (a directory
    // holds at most one package.json); see AC-007's doc comment in
    // commands/version.rs for the full explanation.
    let owner = PackageId::Bare("hybrid".to_string());
    let group_name = GroupName("hybrid-group".to_string());
    let group = ws
        .config
        .groups
        .fixed
        .get_mut(&group_name)
        .expect("hybrid-group must exist as a real Fixed group after Workspace::load");
    group
        .members
        .push(callisto_graph::config::groups::GroupMember::PlatformManifest {
            owner: owner.clone(),
            role: ManifestRole::Platform {
                platform: "darwin".to_string(),
                arch: "arm64".to_string(),
                abi: None,
            },
            path: std::path::PathBuf::from("crates/hybrid-darwin/package.json"),
            name: "@myorg/hybrid-darwin-arm64".to_string(),
        });

    // Discovery (Workspace::load) already opened+cached the owner's
    // Cargo.toml (publish_targets/iter_dependencies scans) and both real
    // platform manifests. Reset here so the count below measures only
    // plan_version's own opens.
    callisto_manifests::reset_open_call_count();

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        strict_graph: false,
        allow_empty_changesets: true,
    };
    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    assert_eq!(
        plan.platform_writes.len(),
        2,
        "expected one PlatformWrite per platform sibling (two total), got: {:?}",
        plan.platform_writes
    );
    assert_eq!(
        plan.optional_dep_updates.len(),
        1,
        "both siblings' optional-dep updates must merge into one entry for the owner's path, got: {:?}",
        plan.optional_dep_updates
    );
    assert_eq!(
        plan.optional_dep_updates[0].updates.len(),
        2,
        "the merged entry must carry both (name, version) pairs, got: {:?}",
        plan.optional_dep_updates[0].updates
    );

    // plan_version's own opens: each platform sibling's own manifest is
    // opened once (2, unavoidable -- each is a distinct real file) PLUS the
    // owner's manifest opened AT MOST ONCE total across both iterations
    // (routed through the shared read-only manifest_cache), not once per
    // sibling. Before the fix this would be 4 (2 platform + 2 owner
    // re-opens); the count below proves the owner check no longer scales
    // with the number of platform siblings.
    assert_eq!(
        callisto_manifests::open_call_count(),
        2,
        "plan_version must open each platform sibling's own manifest once (2 total) and reuse the \
         cached owner manifest for both siblings' optional-dep checks, not re-open it once per sibling"
    );
}
