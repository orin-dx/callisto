//! AC-009 check (c): apply_version_plan must open each distinct manifest
//! path exactly once. Isolated in its own integration-test binary because
//! callisto_manifests::open_call_count is a process-global counter that
//! other, non-#[serial] tests in a shared binary would pollute (see
//! crates/callisto-graph/tests/manifest_cache_test.rs for the precedent).

use std::path::PathBuf;

use callisto_graph::apply::{apply_version_plan, ApplyOptions};
use callisto_graph::cascade::{DepWriteTarget, RewriteKey, SpecRewrite};
use callisto_graph::plan::{PlannedBump, VersionPlan, VersionWriteTarget};
use callisto_model::{
    ApplyPermit, CommandError, CommandOutput, CommandRunner, DepKind, DepSpec, Ecosystem, PackageId, Severity, Version,
    VersionGrammar, VersionReq,
};
use serial_test::serial;

struct NoopRunner;

impl CommandRunner for NoopRunner {
    fn run(&self, _program: &str, _args: &[&str], _cwd: &std::path::Path) -> Result<CommandOutput, CommandError> {
        Ok(CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

#[test]
#[serial]
fn apply_version_plan_opens_each_distinct_manifest_path_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"my-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"1.0.0\"\n",
    )
    .unwrap();
    let other_dir = root.join("other-pkg");
    std::fs::create_dir_all(&other_dir).unwrap();
    std::fs::write(
        other_dir.join("Cargo.toml"),
        "[package]\nname = \"other-pkg\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"1.0.0\"\n",
    )
    .unwrap();

    let bump_manifest = PathBuf::from("Cargo.toml");
    let rewrite_manifest = PathBuf::from("other-pkg/Cargo.toml");

    let plan = VersionPlan {
        bumps: vec![PlannedBump {
            package: PackageId::parse("cargo:my-crate").unwrap(),
            from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            to: Version::parse("1.1.0", VersionGrammar::SemVer).unwrap(),
            severity: Severity::Minor,
            governed_by: None,
            reason: None,
            writes: vec![VersionWriteTarget::Manifest(bump_manifest.clone())],
        }],
        rewrites: vec![SpecRewrite {
            key: RewriteKey {
                target: DepWriteTarget::Manifest(rewrite_manifest.clone()),
                name: "helper".to_string(),
                kind: Some(DepKind::Runtime),
            },
            dependency: PackageId::parse("cargo:helper").unwrap(),
            from: DepSpec::Range(
                VersionReq::parse("^1.0.0", Ecosystem::Cargo).unwrap(),
                "^1.0.0".to_string(),
            ),
            to: DepSpec::Range(
                VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
                "^1.1.0".to_string(),
            ),
        }],
        ..Default::default()
    };

    let permit = ApplyPermit::force_for_tests();
    let opts = ApplyOptions::default();

    callisto_manifests::reset_open_call_count();
    let result = apply_version_plan(root, &plan, &NoopRunner, &opts, &permit);
    assert!(result.is_ok(), "apply_version_plan should succeed: {result:?}");

    assert_eq!(
        callisto_manifests::open_call_count(),
        2,
        "apply_version_plan must open each of the 2 distinct manifest paths exactly once"
    );
}

#[test]
#[serial]
fn apply_version_plan_batches_open_persist_and_dedupes_staged_for_shared_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"my-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"1.0.0\"\n",
    )
    .unwrap();

    let manifest_rel = PathBuf::from("Cargo.toml");
    let plan = VersionPlan {
        bumps: vec![PlannedBump {
            package: PackageId::parse("cargo:my-crate").unwrap(),
            from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            to: Version::parse("1.1.0", VersionGrammar::SemVer).unwrap(),
            severity: Severity::Minor,
            governed_by: None,
            reason: None,
            writes: vec![VersionWriteTarget::Manifest(manifest_rel.clone())],
        }],
        rewrites: vec![SpecRewrite {
            key: RewriteKey {
                target: DepWriteTarget::Manifest(manifest_rel.clone()),
                name: "helper".to_string(),
                kind: Some(DepKind::Runtime),
            },
            dependency: PackageId::parse("cargo:helper").unwrap(),
            from: DepSpec::Range(
                VersionReq::parse("^1.0.0", Ecosystem::Cargo).unwrap(),
                "^1.0.0".to_string(),
            ),
            to: DepSpec::Range(
                VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
                "^1.1.0".to_string(),
            ),
        }],
        ..Default::default()
    };

    let permit = ApplyPermit::force_for_tests();
    let opts = ApplyOptions::default();

    callisto_manifests::reset_open_call_count();
    callisto_manifests::reset_persist_call_count();
    let outcome =
        apply_version_plan(root, &plan, &NoopRunner, &opts, &permit).expect("apply_version_plan should succeed");

    assert_eq!(
        callisto_manifests::open_call_count(),
        1,
        "N=2 Manifest-trait entries sharing one path must open that path exactly once, not N times"
    );
    assert_eq!(
        callisto_manifests::persist_call_count(),
        1,
        "N=2 Manifest-trait entries sharing one path must persist that path exactly once, not N times"
    );
    assert_eq!(
        outcome.staged.iter().filter(|p| **p == manifest_rel).count(),
        1,
        "the shared path must appear in staged exactly once, not N times; staged: {:?}",
        outcome.staged
    );
}

#[test]
#[serial]
fn mixed_routing_root_cargo_toml_excludes_plain_dependencies_from_batching() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\".\"]\n\n[package]\nname = \"root-pkg\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n[dependencies]\nplain-a = \"1.0.0\"\nplain-b = \"1.0.0\"\n\n[workspace.dependencies]\ninherited-c = \"1.0.0\"\n",
    )
    .unwrap();

    let manifest_rel = PathBuf::from("Cargo.toml");
    let plan = VersionPlan {
        rewrites: vec![
            SpecRewrite {
                key: RewriteKey {
                    target: DepWriteTarget::Manifest(manifest_rel.clone()),
                    name: "plain-a".to_string(),
                    kind: Some(DepKind::Runtime),
                },
                dependency: PackageId::parse("cargo:plain-a").unwrap(),
                from: DepSpec::Range(
                    VersionReq::parse("^1.0.0", Ecosystem::Cargo).unwrap(),
                    "^1.0.0".to_string(),
                ),
                to: DepSpec::Range(
                    VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
                    "^1.1.0".to_string(),
                ),
            },
            SpecRewrite {
                key: RewriteKey {
                    target: DepWriteTarget::Manifest(manifest_rel.clone()),
                    name: "plain-b".to_string(),
                    kind: Some(DepKind::Runtime),
                },
                dependency: PackageId::parse("cargo:plain-b").unwrap(),
                from: DepSpec::Range(
                    VersionReq::parse("^1.0.0", Ecosystem::Cargo).unwrap(),
                    "^1.0.0".to_string(),
                ),
                to: DepSpec::Range(
                    VersionReq::parse("^1.2.0", Ecosystem::Cargo).unwrap(),
                    "^1.2.0".to_string(),
                ),
            },
            SpecRewrite {
                key: RewriteKey {
                    target: DepWriteTarget::CargoWorkspaceDependency {
                        root_manifest: manifest_rel.clone(),
                    },
                    name: "inherited-c".to_string(),
                    kind: Some(DepKind::Runtime),
                },
                dependency: PackageId::parse("cargo:inherited-c").unwrap(),
                from: DepSpec::Range(
                    VersionReq::parse("^1.0.0", Ecosystem::Cargo).unwrap(),
                    "^1.0.0".to_string(),
                ),
                to: DepSpec::Range(
                    VersionReq::parse("^1.3.0", Ecosystem::Cargo).unwrap(),
                    "^1.3.0".to_string(),
                ),
            },
        ],
        ..Default::default()
    };

    let permit = ApplyPermit::force_for_tests();
    let opts = ApplyOptions::default();
    callisto_manifests::reset_open_call_count();
    let result = apply_version_plan(root, &plan, &NoopRunner, &opts, &permit);
    assert!(result.is_ok(), "apply_version_plan should succeed: {result:?}");

    assert_eq!(
        callisto_manifests::open_call_count(),
        2,
        "the two plain DepWriteTarget::Manifest entries must each be opened separately via the excluded/unbatched path, not folded into one batched group"
    );

    let on_disk = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(on_disk.contains("plain-a = \"^1.1.0\""));
    assert!(on_disk.contains("plain-b = \"^1.2.0\""));
    assert!(on_disk.contains("inherited-c = \"^1.3.0\""));
}

/// AC-016 (persist_call_count half): a batched group where the bump is
/// skipped (already at target) but a rewrite succeeds must still cause
/// exactly one persist call -- the skipped bump must not suppress the
/// persist a successful rewrite on the same path requires.
///
/// DEVIATION FROM AC-016'S LITERAL FILE-PLACEMENT WORDING (documented, per
/// plan T13): AC-016's text places this assertion in apply.rs's
/// `#[cfg(test)]` module, but PERSIST_CALL_COUNT is a process-global
/// counter and apply.rs's own `--lib` module has 9+ other non-#[serial]
/// tests that would race it -- the same hazard this file's module doc
/// already isolates OPEN_CALL_COUNT from. This test lives here instead,
/// reusing this file's already-imported types.
#[test]
#[serial]
fn batched_group_skipped_bump_still_increments_persist_call_count_by_one() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"my-crate\"\nversion = \"1.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"1.0.0\"\n",
    )
    .unwrap();

    let manifest_rel = PathBuf::from("Cargo.toml");
    let plan = VersionPlan {
        bumps: vec![PlannedBump {
            package: PackageId::parse("cargo:my-crate").unwrap(),
            from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            to: Version::parse("1.1.0", VersionGrammar::SemVer).unwrap(),
            severity: Severity::Minor,
            governed_by: None,
            reason: None,
            writes: vec![VersionWriteTarget::Manifest(manifest_rel.clone())],
        }],
        rewrites: vec![SpecRewrite {
            key: RewriteKey {
                target: DepWriteTarget::Manifest(manifest_rel.clone()),
                name: "helper".to_string(),
                kind: Some(DepKind::Runtime),
            },
            dependency: PackageId::parse("cargo:helper").unwrap(),
            from: DepSpec::Range(
                VersionReq::parse("^1.0.0", Ecosystem::Cargo).unwrap(),
                "^1.0.0".to_string(),
            ),
            to: DepSpec::Range(
                VersionReq::parse("^1.2.0", Ecosystem::Cargo).unwrap(),
                "^1.2.0".to_string(),
            ),
        }],
        ..Default::default()
    };

    let permit = ApplyPermit::force_for_tests();
    let opts = ApplyOptions::default();
    callisto_manifests::reset_persist_call_count();
    let result = apply_version_plan(root, &plan, &NoopRunner, &opts, &permit);
    assert!(result.is_ok(), "apply_version_plan should succeed: {result:?}");

    assert_eq!(
        callisto_manifests::persist_call_count(),
        1,
        "a skipped bump must not suppress the persist a successful rewrite on the same path requires"
    );
}

/// Perf-fix proof: a workspace root `Cargo.toml` written exclusively through
/// `WorkspaceCargoResolver` (a `CargoWorkspacePackage` version bump plus
/// three `CargoWorkspaceDependency` rewrites, all sharing the same root
/// manifest, no competing `Manifest`-trait write to that same physical
/// file) must be loaded and persisted exactly once per `apply_version_plan`
/// call, not once per write (N=4 here).
///
/// `resolver_load_call_count()` is asserted at 2, not 1:
/// `OpenContext::for_workspace_root` (called unconditionally at the top of
/// `apply_version_plan` whenever the workspace root has a `Cargo.toml`, to
/// build the Cargo-inheritance context) accounts for one load; the
/// resolver-batched loop's own single `WorkspaceCargoResolver::load`
/// accounts for the other. Neither count scales with the number of
/// resolver-routed writes -- before this fix, the four writes below would
/// have produced 1 (incidental) + 4 (one load per write) = 5 loads and 4
/// persists; after, they produce 2 loads and exactly 1 persist.
#[test]
#[serial]
fn resolver_batched_root_manifest_opens_once_and_persists_once_for_bump_plus_multiple_rewrites() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"1.0.0\"\n\n[workspace.dependencies]\ndep-a = \"1.0.0\"\ndep-b = \"1.0.0\"\ndep-c = \"1.0.0\"\n",
    )
    .unwrap();

    let manifest_rel = PathBuf::from("Cargo.toml");
    let plan = VersionPlan {
        bumps: vec![PlannedBump {
            package: PackageId::parse("cargo:workspace-root").unwrap(),
            from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            to: Version::parse("1.1.0", VersionGrammar::SemVer).unwrap(),
            severity: Severity::Minor,
            governed_by: None,
            reason: None,
            writes: vec![VersionWriteTarget::CargoWorkspacePackage {
                root_manifest: manifest_rel.clone(),
            }],
        }],
        rewrites: vec!["dep-a", "dep-b", "dep-c"]
            .into_iter()
            .map(|name| SpecRewrite {
                key: RewriteKey {
                    target: DepWriteTarget::CargoWorkspaceDependency {
                        root_manifest: manifest_rel.clone(),
                    },
                    name: name.to_string(),
                    kind: None,
                },
                dependency: PackageId::parse(&format!("cargo:{name}")).unwrap(),
                from: DepSpec::Range(
                    VersionReq::parse("^1.0.0", Ecosystem::Cargo).unwrap(),
                    "^1.0.0".to_string(),
                ),
                to: DepSpec::Range(
                    VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
                    "^1.1.0".to_string(),
                ),
            })
            .collect(),
        ..Default::default()
    };

    let permit = ApplyPermit::force_for_tests();
    let opts = ApplyOptions::default();
    callisto_manifests::reset_resolver_load_call_count();
    callisto_manifests::reset_resolver_persist_call_count();
    let outcome =
        apply_version_plan(root, &plan, &NoopRunner, &opts, &permit).expect("apply_version_plan should succeed");

    assert_eq!(
        callisto_manifests::resolver_load_call_count(),
        2,
        "1 incidental OpenContext::for_workspace_root load + 1 resolver-batched-loop load, \
         regardless of how many resolver-routed writes (4 here) target the shared root manifest"
    );
    assert_eq!(
        callisto_manifests::resolver_persist_call_count(),
        1,
        "N=4 resolver-routed writes (1 bump + 3 rewrites) sharing one root manifest must persist \
         that manifest exactly once, not N times"
    );

    let on_disk = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(on_disk.contains("version = \"1.1.0\""), "got:\n{on_disk}");
    assert!(on_disk.contains("dep-a = \"^1.1.0\""), "got:\n{on_disk}");
    assert!(on_disk.contains("dep-b = \"^1.1.0\""), "got:\n{on_disk}");
    assert!(on_disk.contains("dep-c = \"^1.1.0\""), "got:\n{on_disk}");

    assert_eq!(
        outcome.staged.iter().filter(|p| **p == manifest_rel).count(),
        1,
        "the shared root manifest must appear in staged exactly once, not N times; staged: {:?}",
        outcome.staged
    );
}

/// Contrast case: when the root manifest ALSO receives a `Manifest`-trait
/// write (the mixed-routing hazard `classification.excluded` guards
/// against), resolver-routed writes to that same path must NOT be batched
/// -- each is still loaded and persisted individually, exactly as before
/// this perf fix.
#[test]
#[serial]
fn mixed_routing_root_manifest_still_persists_resolver_writes_individually() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\".\"]\n\n[package]\nname = \"root-pkg\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n[dependencies]\nplain-dep = \"1.0.0\"\n\n[workspace.dependencies]\ndep-a = \"1.0.0\"\ndep-b = \"1.0.0\"\n",
    )
    .unwrap();

    let manifest_rel = PathBuf::from("Cargo.toml");
    let mut rewrites: Vec<SpecRewrite> = vec!["dep-a", "dep-b"]
        .into_iter()
        .map(|name| SpecRewrite {
            key: RewriteKey {
                target: DepWriteTarget::CargoWorkspaceDependency {
                    root_manifest: manifest_rel.clone(),
                },
                name: name.to_string(),
                kind: None,
            },
            dependency: PackageId::parse(&format!("cargo:{name}")).unwrap(),
            from: DepSpec::Range(
                VersionReq::parse("^1.0.0", Ecosystem::Cargo).unwrap(),
                "^1.0.0".to_string(),
            ),
            to: DepSpec::Range(
                VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
                "^1.1.0".to_string(),
            ),
        })
        .collect();
    // A plain (non-inherited) `DepWriteTarget::Manifest` rewrite targeting
    // this SAME physical path is what actually makes this mixed-routing --
    // `classify_manifest_writes` classifies purely on which write targets a
    // *plan* contains for a path, not on what the file's on-disk content
    // could theoretically support.
    rewrites.push(SpecRewrite {
        key: RewriteKey {
            target: DepWriteTarget::Manifest(manifest_rel.clone()),
            name: "plain-dep".to_string(),
            kind: Some(DepKind::Runtime),
        },
        dependency: PackageId::parse("cargo:plain-dep").unwrap(),
        from: DepSpec::Range(
            VersionReq::parse("^1.0.0", Ecosystem::Cargo).unwrap(),
            "^1.0.0".to_string(),
        ),
        to: DepSpec::Range(
            VersionReq::parse("^1.2.0", Ecosystem::Cargo).unwrap(),
            "^1.2.0".to_string(),
        ),
    });
    let plan = VersionPlan {
        rewrites,
        ..Default::default()
    };

    let permit = ApplyPermit::force_for_tests();
    let opts = ApplyOptions::default();
    callisto_manifests::reset_resolver_load_call_count();
    callisto_manifests::reset_resolver_persist_call_count();
    let result = apply_version_plan(root, &plan, &NoopRunner, &opts, &permit);
    assert!(result.is_ok(), "apply_version_plan should succeed: {result:?}");

    assert_eq!(
        callisto_manifests::resolver_load_call_count(),
        3,
        "1 incidental OpenContext::for_workspace_root load + 2 individual loads (one per \
         resolver-routed rewrite), since this root manifest is mixed-routed and therefore excluded \
         from resolver batching entirely"
    );
    assert_eq!(
        callisto_manifests::resolver_persist_call_count(),
        2,
        "mixed-routing root manifests must persist each resolver-routed write individually, not batch them"
    );
}
