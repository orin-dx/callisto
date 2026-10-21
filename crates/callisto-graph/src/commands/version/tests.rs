use std::path::Path;

use super::{
    check_partial_run, head_has_pending_changeset, head_object_exists, manifest_blob_at, plan_version, VersionOptions,
};
use crate::error::GraphError;
use crate::infer::NoInference;
use crate::locate::IgnoreWalkLocator;
use crate::Workspace;
use callisto_fixtures::git::GitRunner;
use callisto_model::{CommandError, CommandOutput, CommandRunner};

struct IndexInspectionFailureRunner {
    failed_args: Vec<String>,
}

impl CommandRunner for IndexInspectionFailureRunner {
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
        if program == "git" && args.iter().copied().eq(self.failed_args.iter().map(String::as_str)) {
            return Ok(CommandOutput {
                exit_code: Some(128),
                stdout: String::new(),
                stderr: "fatal: unable to read index".to_string(),
            });
        }
        GitRunner.run(program, args, cwd)
    }
}

fn git_init_with_commit(root: &Path) {
    callisto_fixtures::git::init_repo(root);
    std::fs::write(root.join(".gitkeep"), "").unwrap();
    for args in [vec!["add", "."], vec!["commit", "-q", "-m", "init"]] {
        std::process::Command::new("git")
            .args(&args)
            .current_dir(root)
            .output()
            .expect("git must be installed");
    }
}

fn tag(root: &Path, name: &str) {
    std::process::Command::new("git")
        .args(["-c", "tag.gpgSign=false", "tag", "-m", "release", name])
        .current_dir(root)
        .output()
        .expect("git must be installed");
}

fn commit_all(root: &Path, message: &str) {
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .output()
        .expect("git add");
    std::process::Command::new("git")
        .args(["commit", "-q", "-m", message])
        .current_dir(root)
        .output()
        .expect("git commit");
}

fn write_pkg_a(root: &Path, version: &str) {
    std::fs::create_dir_all(root.join("pkg-a")).unwrap();
    std::fs::write(
        root.join("pkg-a/Cargo.toml"),
        format!("[package]\nname = \"pkg-a\"\nversion = \"{version}\"\n"),
    )
    .unwrap();
}

fn write_changeset(root: &Path, name: &str) {
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(root.join(".changeset").join(name), "---\npkg-a: minor\n---\n\nfeat\n").unwrap();
}

/// A pending changeset at `HEAD` plus a manifest already bumped past `HEAD` on disk must be refused.
#[test]
fn check_partial_run_refuses_when_disk_is_ahead_of_head_with_a_pending_changeset() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    write_pkg_a(root, "1.0.0");
    commit_all(root, "add pkg-a");
    write_changeset(root, "a.md");
    commit_all(root, "cs");

    // Never committed: simulates apply having bumped the manifest on disk before the run crashed.
    write_pkg_a(root, "1.1.0");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let result = check_partial_run(&ws);
    assert!(
            matches!(result, Err(GraphError::PartialVersionRun { .. })),
            "must refuse a workspace with a pending-at-HEAD changeset and a manifest already ahead of HEAD, got: {result:?}"
        );
}

/// A changeset pending at `HEAD` with disk exactly matching `HEAD` must never be refused.
#[test]
fn check_partial_run_allows_an_ordinary_pending_changeset() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    write_pkg_a(root, "1.0.0");
    commit_all(root, "add pkg-a");
    write_changeset(root, "a.md");
    commit_all(root, "cs");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    assert!(
        check_partial_run(&ws).is_ok(),
        "an ordinary pending changeset with disk matching HEAD must not be refused"
    );
}

#[test]
fn check_partial_run_propagates_fatal_staged_manifest_inspection_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);
    write_pkg_a(root, "1.0.0");
    commit_all(root, "add pkg-a");
    write_pkg_a(root, "1.1.0");
    std::process::Command::new("git")
        .args(["add", "pkg-a/Cargo.toml"])
        .current_dir(root)
        .output()
        .expect("git add must run");

    let locator = IgnoreWalkLocator::new(root);
    let runner = IndexInspectionFailureRunner {
        failed_args: ["diff", "--cached", "--quiet", "--", "pkg-a/Cargo.toml"]
            .map(str::to_string)
            .to_vec(),
    };
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");
    let error = check_partial_run(&ws).unwrap_err();
    assert!(matches!(
            &error,
            GraphError::Command(CommandError::Failed {
                program,
                exit_code: Some(128),
                stderr
            }) if program == "git" && stderr.contains("unable to read index")
    ));
}

#[test]
fn check_partial_run_propagates_fatal_head_sha_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);
    write_pkg_a(root, "1.0.0");
    commit_all(root, "add package");
    let locator = IgnoreWalkLocator::new(root);
    let runner = IndexInspectionFailureRunner {
        failed_args: ["rev-parse", "HEAD"].map(str::to_string).to_vec(),
    };
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");
    let error = check_partial_run(&ws).expect_err("fatal HEAD lookup must not be mistaken for an unborn repo");
    assert!(matches!(
        error,
        GraphError::Vcs(callisto_model::vcs::VcsError::Git(message))
            if message.contains("git rev-parse HEAD")
    ));
}

#[test]
fn check_partial_run_propagates_fatal_historical_git_read_errors() {
    for failed_command in ["rev-parse", "ls-tree", "show"] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);
        write_pkg_a(root, "1.0.0");
        write_changeset(root, "a.md");
        commit_all(root, "add package and inputs");

        let locator = IgnoreWalkLocator::new(root);
        let changeset_dir = {
            let ws = Workspace::load(root.to_path_buf(), &locator, &GitRunner).expect("workspace must load");
            crate::commands::release_decision::normalized_git_path(&ws.config.changesets_dir).unwrap()
        };
        let changeset_spec = format!("HEAD:{changeset_dir}");
        let failed_args = match failed_command {
            "rev-parse" => vec![
                "rev-parse".to_string(),
                "--verify".to_string(),
                "--quiet".to_string(),
                changeset_spec.clone(),
            ],
            "ls-tree" => vec!["ls-tree".to_string(), "--name-only".to_string(), changeset_spec.clone()],
            _ => vec!["show".to_string(), "HEAD:pkg-a/Cargo.toml".to_string()],
        };
        let runner = IndexInspectionFailureRunner {
            failed_args: failed_args.clone(),
        };
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");
        let result = match failed_command {
            "rev-parse" => head_object_exists(&ws, &changeset_spec).map(|_| ()),
            "ls-tree" => head_has_pending_changeset(&ws, "HEAD").map(|_| ()),
            _ => manifest_blob_at(&ws, "HEAD", "pkg-a/Cargo.toml").map(|_| ()),
        };
        let error = result
            .err()
            .unwrap_or_else(|| panic!("expected failure for {failed_args:?}"));
        assert!(
            matches!(
                &error,
                GraphError::Command(CommandError::Failed {
                    program,
                    exit_code: Some(128),
                    stderr
                }) if program == "git" && stderr.contains("unable to read index")
            ),
            "unexpected result for {failed_args:?}: {error:?}"
        );
    }
}

/// A repo with no commits yet has no `HEAD` to compare against.
#[test]
fn check_partial_run_allows_a_repo_with_no_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    callisto_fixtures::git::init_repo(root);
    write_pkg_a(root, "1.0.0");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    assert!(check_partial_run(&ws).is_ok(), "an unborn repo must not be refused");
}

/// A Fixed group whose
/// released members' base versions parse under the same grammar but
/// disagree in value must make plan_version return
/// Err(GraphError::FixedGroupDivergent) before run_cascade ever
/// executes -- exercised through plan_version itself, not a direct
/// pre_mutation_checks call.
#[test]
fn version_rejects_divergent_fixed_group_via_real_call_path() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    for (name, version) in [("pkg-a", "1.0.0"), ("pkg-b", "1.1.0")] {
        std::fs::create_dir_all(root.join(name)).unwrap();
        std::fs::write(
            root.join(name).join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\n"),
        )
        .unwrap();
    }
    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"ab\"\nmembers = [\"cargo/pkg-a\", \"cargo/pkg-b\"]\n",
    )
    .unwrap();
    commit_all(root, "add packages");
    tag(root, "pkg-a@1.0.0");
    tag(root, "pkg-b@1.1.0");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let result = plan_version(&ws, &inference, &opts);

    assert!(
            matches!(result, Err(GraphError::FixedGroupDivergent { .. })),
            "plan_version must return Err(FixedGroupDivergent) for a Fixed group with divergent released-member versions, got: {result:?}"
        );
}

/// A Fixed group whose released
/// members use different version grammars (SemVer vs PEP 440) must
/// return Err(GraphError::GroupGrammarMismatch) via plan_version's real
/// call path, before run_cascade executes.
#[test]
fn version_rejects_grammar_mismatched_fixed_group_via_real_call_path() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    std::fs::create_dir_all(root.join("pkg-a")).unwrap();
    std::fs::write(
        root.join("pkg-a/Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join("pkg-py")).unwrap();
    std::fs::write(
        root.join("pkg-py/pyproject.toml"),
        "[project]\nname = \"pkg-py\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"mixed\"\nmembers = [\"cargo/pkg-a\", \"pypi/pkg-py\"]\n",
    )
    .unwrap();
    commit_all(root, "add packages");
    tag(root, "pkg-a@1.0.0");
    tag(root, "pkg-py@1.0.0");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let result = plan_version(&ws, &inference, &opts);

    assert!(
            matches!(result, Err(GraphError::GroupGrammarMismatch { .. })),
            "plan_version must return Err(GroupGrammarMismatch) for a Fixed group mixing SemVer and PEP 440 released members, got: {result:?}"
        );
}

/// A Fixed group's napi package.json existing at the
/// conventional path but containing malformed JSON must surface as
/// Err(GraphError::Manifest(ManifestError::Parse { .. })) from
/// plan_version, with no VersionPlan produced.
#[test]
fn version_propagates_malformed_napi_package_json_as_manifest_parse_error() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    std::fs::create_dir_all(root.join("my-lib")).unwrap();
    std::fs::write(
        root.join("my-lib/Cargo.toml"),
        "[package]\nname = \"my-lib\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    // Truncated / malformed JSON at the conventional napi path.
    std::fs::write(
        root.join("my-lib/package.json"),
        "{\"name\":\"my-lib\",\"napi\":{\"targets\":[",
    )
    .unwrap();
    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"solo\"\nmembers = [\"cargo/my-lib\"]\n",
    )
    .unwrap();
    commit_all(root, "add package");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let result = plan_version(&ws, &inference, &opts);

    assert!(
        matches!(
            result,
            Err(GraphError::Manifest(callisto_model::ManifestError::Parse {
                format: callisto_model::ManifestFormat::PackageJson,
                ..
            }))
        ),
        "plan_version must propagate malformed napi package.json as Err(ManifestError::Parse); got: {result:?}"
    );
}

/// GroupCheckOutcome.diagnostics returned by pre_mutation_checks
/// must be merged into VersionPlan.diagnostics, not discarded.
#[test]
fn version_merges_group_check_diagnostics_into_plan() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    std::fs::create_dir_all(root.join("my-lib")).unwrap();
    std::fs::write(
        root.join("my-lib/Cargo.toml"),
        "[package]\nname = \"my-lib\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    // Declares a napi target not present among the fixed group's own
    // members -- napi_drift reports NapiTargetAddedNotInMembers for this.
    std::fs::write(
        root.join("my-lib/package.json"),
        "{\"name\":\"my-lib\",\"napi\":{\"targets\":[\"aarch64-apple-darwin\"]}}",
    )
    .unwrap();
    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"solo\"\nmembers = [\"cargo/my-lib\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(root.join(".changeset/bump.md"), "---\n\"my-lib\": patch\n---\n\nfix.\n").unwrap();
    commit_all(root, "add package");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    assert!(
        plan.diagnostics
            .iter()
            .any(|d| matches!(d.code, callisto_model::DiagnosticCode::NapiTargetAddedNotInMembers)),
        "VersionPlan.diagnostics must include the napi-drift diagnostic produced by pre_mutation_checks; got: {:?}",
        plan.diagnostics
    );
}

/// A package with a bump this run, `pkg.changelog` set, no pending
/// changeset, and a BumpReason with no direct ChangeSource mapping
/// (PreRelease) must still produce exactly one ChangelogWrite entry via
/// the generic 'Version bump ({severity})' fallback -- plan_version must
/// not panic or error.
#[test]
fn version_prerelease_exit_falls_back_to_generic_changeset_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    std::fs::create_dir_all(root.join("pkg-a")).unwrap();
    std::fs::write(
        root.join("pkg-a/Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.1.0-alpha.1\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(
        root.join(".changeset/pre.json"),
        r#"{"mode":"exit","tag":"alpha","initialVersions":{},"changesets":[]}"#,
    )
    .unwrap();
    commit_all(root, "add package");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    let pkg_a = callisto_model::PackageId::parse("pkg-a").unwrap();
    let write = plan
        .changelog_writes
        .iter()
        .find(|w| w.input.package == pkg_a)
        .expect("pkg-a must have a ChangelogWrite");

    assert_eq!(
        write.input.entries.len(),
        1,
        "expected exactly 1 entry, got {:?}",
        write.input.entries
    );
    match &write.input.entries[0].source {
        crate::changelog::ChangeSource::Changeset { filename, summary } => {
            assert_eq!(filename, "");
            assert_eq!(summary, "Version bump (patch)");
        }
        other => panic!("expected ChangeSource::Changeset fallback, got {other:?}"),
    }
}

/// A Fixed group with an owner Package member that
/// receives a real PlannedBump this run, plus a sibling
/// GroupMember::PlatformManifest member (a dual-manifest hybrid-root npm
/// package.json with non-empty os/cpu arrays living in the same
/// directory as the owner's Cargo.toml), must produce exactly one
/// PlatformWrite whose `manifest` is the platform manifest's path,
/// whose `version` equals the owner's bump target, and whose `from`
/// equals the platform manifest's on-disk current version at plan time.
#[test]
fn plan_version_emits_platform_write_for_bumped_owner_with_platform_sibling() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    std::fs::create_dir_all(root.join("crates/hybrid")).unwrap();
    std::fs::write(
        root.join("crates/hybrid/Cargo.toml"),
        "[package]\nname = \"hybrid\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("crates/hybrid/package.json"),
        r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.9.0","os":["darwin"],"cpu":["arm64"]}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"cargo/hybrid\", \"@myorg/hybrid-darwin-arm64\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(root.join(".changeset/bump.md"), "---\n\"hybrid\": patch\n---\n\nfix.\n").unwrap();
    commit_all(root, "add hybrid package");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    assert_eq!(
        plan.platform_writes.len(),
        1,
        "expected exactly one PlatformWrite, got: {:?}",
        plan.platform_writes
    );
    let pw = &plan.platform_writes[0];
    assert_eq!(
        pw.manifest,
        Path::new("crates/hybrid/package.json"),
        "PlatformWrite.manifest must be the platform manifest's path"
    );
    let owner_bump = plan
        .bumps
        .iter()
        .find(|b| b.package.name() == "hybrid")
        .expect("hybrid must have a planned bump");
    assert_eq!(
        pw.version, owner_bump.to,
        "PlatformWrite.version must equal the owner's bump target"
    );
    assert_eq!(
        pw.from.to_string(),
        "0.9.0",
        "PlatformWrite.from must equal the platform manifest's on-disk current version"
    );
}

/// When no Fixed group anywhere has any
/// GroupMember::PlatformManifest members, plan_version's
/// platform_writes and optional_dep_updates must both be empty --
/// no regression for workspaces without platform packages.
#[test]
fn plan_version_platform_writes_empty_when_no_platform_members_exist() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    std::fs::create_dir_all(root.join("my-lib")).unwrap();
    std::fs::write(
        root.join("my-lib/Cargo.toml"),
        "[package]\nname = \"my-lib\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"solo\"\nmembers = [\"cargo/my-lib\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(root.join(".changeset/bump.md"), "---\n\"my-lib\": patch\n---\n\nfix.\n").unwrap();
    commit_all(root, "add package");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    assert!(
        plan.platform_writes.is_empty(),
        "platform_writes must be empty when no PlatformManifest members exist; got: {:?}",
        plan.platform_writes
    );
    assert!(
        plan.optional_dep_updates.is_empty(),
        "optional_dep_updates must be empty when no PlatformManifest members exist; got: {:?}",
        plan.optional_dep_updates
    );
}

/// When a Fixed group's owner package receives no version bump
/// this run, plan_version must produce no PlatformWrite for that
/// group's GroupMember::PlatformManifest members, even though the
/// platform manifest exists on disk -- platform writes are driven
/// strictly by an actual owner PlannedBump, never speculatively.
#[test]
fn plan_version_no_platform_write_when_owner_not_bumped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    std::fs::create_dir_all(root.join("crates/hybrid")).unwrap();
    std::fs::write(
        root.join("crates/hybrid/Cargo.toml"),
        "[package]\nname = \"hybrid\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("crates/hybrid/package.json"),
        r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.9.0","os":["darwin"],"cpu":["arm64"]}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"cargo/hybrid\", \"@myorg/hybrid-darwin-arm64\"]\n",
    )
    .unwrap();
    // No changeset for "hybrid" -- outcome.severities for hybrid is
    // Severity::None, so no PlannedBump is produced for it.
    commit_all(root, "add hybrid package");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    assert!(
        plan.bumps.iter().all(|b| b.package.name() != "hybrid"),
        "hybrid must not have received a PlannedBump in this scenario; got bumps: {:?}",
        plan.bumps
    );
    assert!(
        plan.platform_writes.is_empty(),
        "platform_writes must be empty when the owner package received no bump; got: {:?}",
        plan.platform_writes
    );
}

/// plan_version's
/// OpenContext/workspace-inheritance resolution logic verified directly
/// against a FIXTURE-CONSTRUCTED GroupMember::PlatformManifest (bypassing
/// walk.rs discovery entirely, since walk.rs:199 only registers npm
/// manifests into IdentityIndex.platform today -- no Cargo.toml can
/// become a GroupMember::PlatformManifest through the real,
/// disk-discovered call path). Confirms plan_version hardcodes
/// ManifestRole::Canonical unconditionally (ignoring the fixture's
/// Platform role) and correctly resolves a Cargo `version.workspace =
/// true` manifest's inherited version from `[workspace.package]`.
#[test]
fn plan_version_platform_write_from_resolves_cargo_workspace_inherited_version() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    // Workspace root Cargo.toml supplying the [workspace.package] version
    // that a `version.workspace = true` platform manifest inherits from.
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\n\n[workspace.package]\nversion = \"3.2.1\"\n",
    )
    .unwrap();

    // The real owner package -- receives a real PlannedBump this run.
    std::fs::create_dir_all(root.join("crates/owner")).unwrap();
    std::fs::write(
        root.join("crates/owner/Cargo.toml"),
        "[package]\nname = \"cargo-owner\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    // The fixture's on-disk platform manifest target -- a maturin-style
    // Cargo.toml inheriting its version from [workspace.package]. This is
    // an ordinary, real, on-disk Cargo.toml; it is NOT registered as a
    // GroupMember::PlatformManifest by walk.rs (that
    // classification is npm-only today) -- it is wired into the group
    // fixture manually below.
    std::fs::create_dir_all(root.join("platform/plat")).unwrap();
    std::fs::write(
        root.join("platform/plat/Cargo.toml"),
        "[package]\nname = \"cargo-owner-linux-x64-gnu\"\nversion.workspace = true\n",
    )
    .unwrap();

    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"cargo-owner-group\"\nmembers = [\"cargo/cargo-owner\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(
        root.join(".changeset/bump.md"),
        "---\n\"cargo-owner\": patch\n---\n\nfix.\n",
    )
    .unwrap();
    commit_all(root, "add owner and fixture platform crate");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let mut ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    // Fixture-construct the GroupMember::PlatformManifest, bypassing
    // walk.rs discovery entirely.
    let owner = callisto_model::PackageId::Bare("cargo-owner".to_string());
    let group_name = callisto_model::GroupName("cargo-owner-group".to_string());
    let group = ws
        .config
        .groups
        .fixed
        .get_mut(&group_name)
        .expect("cargo-owner-group must exist as a real Fixed group after Workspace::load");
    group
        .members
        .push(crate::config::groups::GroupMember::PlatformManifest {
            owner: owner.clone(),
            role: callisto_model::ManifestRole::Platform {
                platform: "linux".to_string(),
                arch: "x64".to_string(),
                abi: Some("gnu".to_string()),
            },
            path: std::path::PathBuf::from("platform/plat/Cargo.toml"),
            name: "cargo-owner-linux-x64-gnu".to_string(),
        });

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    let owner_bump = plan
        .bumps
        .iter()
        .find(|b| b.package == owner)
        .expect("owner must receive a real PlannedBump this run, not a vacuous pass");

    assert_eq!(
        plan.platform_writes.len(),
        1,
        "expected exactly one PlatformWrite for the fixture-constructed Cargo platform member, got: {:?}",
        plan.platform_writes
    );
    let pw = &plan.platform_writes[0];
    assert_eq!(
        pw.manifest,
        Path::new("platform/plat/Cargo.toml"),
        "PlatformWrite.manifest must be the fixture platform manifest's path"
    );
    assert_eq!(
        pw.from.to_string(),
        "3.2.1",
        "PlatformWrite.from must resolve the Cargo workspace-inherited version.workspace = true \
             value from [workspace.package] version, not fail or silently default"
    );
    assert_eq!(
        pw.version, owner_bump.to,
        "PlatformWrite.version must equal the owner's real bump target"
    );
}

/// An owner with two
/// platform siblings (linux-x64-gnu, darwin-arm64), both bumped, must
/// produce one `PlatformWrite` per sibling, not just the first.
///
/// Linux is real/disk-discovered (co-located: `Cargo.toml`+`package.json`
/// sharing a directory). Darwin is fixture-injected, because walk.rs's
/// platform registration (walk.rs:131-227) groups strictly by directory
/// -- a second platform manifest for the same owner can only be
/// disk-discovered from that same directory, and a directory holds at
/// most one `package.json`. So two real disk-discovered siblings under
/// one owner isn't representable via `walk.rs` today (same constraint
/// as the Cargo case). This test instead proves
/// `plan_version`'s own loop -- agnostic to how a member entered
/// `ws.config.groups.fixed` -- iterates every `PlatformManifest` member,
/// not just the first.
#[test]
fn plan_version_emits_platform_write_for_each_of_two_platform_siblings() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    // Real, disk-discovered co-located linux sibling: Cargo.toml and
    // package.json sharing one directory, so `owner` resolves to the
    // Cargo primary_id via walk.rs's co-located promotion.
    std::fs::create_dir_all(root.join("crates/hybrid")).unwrap();
    std::fs::write(
        root.join("crates/hybrid/Cargo.toml"),
        "[package]\nname = \"hybrid\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("crates/hybrid/package.json"),
        r#"{"name":"@myorg/hybrid-linux-x64-gnu","version":"0.9.0","os":["linux"],"cpu":["x64"]}"#,
    )
    .unwrap();

    // Second sibling's on-disk manifest target -- real content, read by
    // plan_version exactly as the first sibling's is, but fixture-wired
    // into the group below since walk.rs cannot discover a second
    // Co-located member under the same owner.
    std::fs::create_dir_all(root.join("crates/hybrid-darwin")).unwrap();
    std::fs::write(
        root.join("crates/hybrid-darwin/package.json"),
        r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.5.0","os":["darwin"],"cpu":["arm64"]}"#,
    )
    .unwrap();

    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"cargo/hybrid\", \"@myorg/hybrid-linux-x64-gnu\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(root.join(".changeset/bump.md"), "---\n\"hybrid\": patch\n---\n\nfix.\n").unwrap();
    commit_all(root, "add hybrid package with linux platform sibling");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let mut ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let owner = callisto_model::PackageId::Bare("hybrid".to_string());
    let group_name = callisto_model::GroupName("hybrid-group".to_string());
    let group = ws
        .config
        .groups
        .fixed
        .get_mut(&group_name)
        .expect("hybrid-group must exist as a real Fixed group after Workspace::load");
    group
        .members
        .push(crate::config::groups::GroupMember::PlatformManifest {
            owner: owner.clone(),
            role: callisto_model::ManifestRole::Platform {
                platform: "darwin".to_string(),
                arch: "arm64".to_string(),
                abi: None,
            },
            path: std::path::PathBuf::from("crates/hybrid-darwin/package.json"),
            name: "@myorg/hybrid-darwin-arm64".to_string(),
        });

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    let owner_bump = plan
        .bumps
        .iter()
        .find(|b| b.package == owner)
        .expect("hybrid must have a planned bump");

    assert_eq!(
        plan.platform_writes.len(),
        2,
        "expected one PlatformWrite per platform sibling (two total), got: {:?}",
        plan.platform_writes
    );

    let linux_pw = plan
        .platform_writes
        .iter()
        .find(|pw| pw.manifest == Path::new("crates/hybrid/package.json"))
        .expect("linux sibling must have its own PlatformWrite entry");
    assert_eq!(linux_pw.from.to_string(), "0.9.0");
    assert_eq!(linux_pw.version, owner_bump.to);

    let darwin_pw = plan
        .platform_writes
        .iter()
        .find(|pw| pw.manifest == Path::new("crates/hybrid-darwin/package.json"))
        .expect("darwin sibling must have its own PlatformWrite entry");
    assert_eq!(darwin_pw.from.to_string(), "0.5.0");
    assert_eq!(darwin_pw.version, owner_bump.to);
}

/// Given the grammar-mismatch scenario (owner with two
/// `GroupMember::PlatformManifest` siblings, both bumped) where the
/// owner's canonical manifest has matching `optionalDependencies`
/// entries for both platform names, `plan_version`'s
/// `VersionPlan.optional_dep_updates` must contain exactly one
/// `OptionalDepUpdate` for the owner's canonical path, with both (name,
/// version) pairs merged in -- never duplicated as separate entries
/// for the same path.
///
/// Fixture construction mirrors
/// `plan_version_emits_platform_write_for_each_of_two_platform_siblings`
/// (see that test's doc comment for why darwin is fixture-injected, not
/// disk-discovered).
#[test]
fn plan_version_merges_two_platform_siblings_optional_dep_updates_into_one_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    // Real, disk-discovered co-located linux sibling: Cargo.toml and
    // package.json sharing one directory. The owner's Cargo.toml
    // declares matching `optional = true` dependencies on BOTH platform
    // members' npm names, so both siblings should produce a merged
    // OptionalDepUpdate entry rather than two separate ones.
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

    // Second sibling's on-disk manifest target -- real content, read by
    // plan_version exactly as the first sibling's is, but fixture-wired
    // into the group below since walk.rs cannot discover a second
    // Co-located member under the same owner.
    std::fs::create_dir_all(root.join("crates/hybrid-darwin")).unwrap();
    std::fs::write(
        root.join("crates/hybrid-darwin/package.json"),
        r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.5.0","os":["darwin"],"cpu":["arm64"]}"#,
    )
    .unwrap();

    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"cargo/hybrid\", \"@myorg/hybrid-linux-x64-gnu\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(root.join(".changeset/bump.md"), "---\n\"hybrid\": patch\n---\n\nfix.\n").unwrap();
    commit_all(root, "add hybrid package with linux platform sibling");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let mut ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let owner = callisto_model::PackageId::Bare("hybrid".to_string());
    let group_name = callisto_model::GroupName("hybrid-group".to_string());
    let group = ws
        .config
        .groups
        .fixed
        .get_mut(&group_name)
        .expect("hybrid-group must exist as a real Fixed group after Workspace::load");
    group
        .members
        .push(crate::config::groups::GroupMember::PlatformManifest {
            owner: owner.clone(),
            role: callisto_model::ManifestRole::Platform {
                platform: "darwin".to_string(),
                arch: "arm64".to_string(),
                abi: None,
            },
            path: std::path::PathBuf::from("crates/hybrid-darwin/package.json"),
            name: "@myorg/hybrid-darwin-arm64".to_string(),
        });

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    let owner_bump = plan
        .bumps
        .iter()
        .find(|b| b.package == owner)
        .expect("hybrid must have a planned bump");

    assert_eq!(
        plan.optional_dep_updates.len(),
        1,
        "expected exactly one merged OptionalDepUpdate for the owner's canonical manifest, \
             not one per platform sibling; got: {:?}",
        plan.optional_dep_updates
    );
    let update = &plan.optional_dep_updates[0];
    assert_eq!(
        update.manifest,
        Path::new("crates/hybrid/Cargo.toml"),
        "OptionalDepUpdate.manifest must be the owner's canonical manifest path"
    );

    let mut updates = update.updates.clone();
    updates.sort_by(|a, b| a.0.cmp(&b.0));
    let mut expected = vec![
        ("@myorg/hybrid-linux-x64-gnu".to_string(), owner_bump.to.clone()),
        ("@myorg/hybrid-darwin-arm64".to_string(), owner_bump.to.clone()),
    ];
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        updates, expected,
        "OptionalDepUpdate.updates must contain both (name, version) pairs merged into the \
             single entry for the owner's manifest path"
    );
}

/// A Fixed group's platform manifest
/// that validated during the walk (so it is a real
/// GroupMember::PlatformManifest) but has been deleted from disk before
/// plan_version runs -- simulating the file having gone missing between
/// workspace load and plan execution -- must surface as Err(GraphError),
/// not panic and not produce a partial VersionPlan.
#[test]
fn plan_version_returns_err_when_platform_manifest_missing_from_disk_at_plan_time() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    std::fs::create_dir_all(root.join("crates/hybrid")).unwrap();
    std::fs::write(
        root.join("crates/hybrid/Cargo.toml"),
        "[package]\nname = \"hybrid\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    let platform_manifest_path = root.join("crates/hybrid/package.json");
    std::fs::write(
        &platform_manifest_path,
        r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.9.0","os":["darwin"],"cpu":["arm64"]}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"cargo/hybrid\", \"@myorg/hybrid-darwin-arm64\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(root.join(".changeset/bump.md"), "---\n\"hybrid\": patch\n---\n\nfix.\n").unwrap();
    commit_all(root, "add hybrid package");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    // Simulate the platform manifest going missing between workspace
    // load and plan_version execution.
    std::fs::remove_file(&platform_manifest_path).unwrap();

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let result = plan_version(&ws, &inference, &opts);

    assert!(
        result.is_err(),
        "plan_version must return Err when the platform manifest is missing from disk at plan time; got: {result:?}"
    );
}

/// A Fixed group's platform
/// manifest that exists on disk but cannot be parsed as valid content
/// for its ecosystem (malformed JSON) must also surface as
/// Err(GraphError), not panic and not produce a partial VersionPlan --
/// distinct from the missing-file disjunct above.
#[test]
fn plan_version_returns_err_when_platform_manifest_content_is_malformed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    std::fs::create_dir_all(root.join("crates/hybrid")).unwrap();
    std::fs::write(
        root.join("crates/hybrid/Cargo.toml"),
        "[package]\nname = \"hybrid\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    let platform_manifest_path = root.join("crates/hybrid/package.json");
    std::fs::write(
        &platform_manifest_path,
        r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.9.0","os":["darwin"],"cpu":["arm64"]}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"cargo/hybrid\", \"@myorg/hybrid-darwin-arm64\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(root.join(".changeset/bump.md"), "---\n\"hybrid\": patch\n---\n\nfix.\n").unwrap();
    commit_all(root, "add hybrid package");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    // Simulate the platform manifest's content becoming malformed
    // (truncated, unbalanced JSON) between workspace load and
    // plan_version execution -- distinct from the file simply vanishing.
    std::fs::write(
        &platform_manifest_path,
        r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.9.0","#,
    )
    .unwrap();

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let result = plan_version(&ws, &inference, &opts);

    assert!(
        result.is_err(),
        "plan_version must return Err when the platform manifest content is malformed; got: {result:?}"
    );
}

/// Given the owner-bumped scenario (owner bumped, sibling
/// GroupMember::PlatformManifest with name N), and the owner's own
/// canonical manifest already declares an optionalDependencies entry
/// whose name exactly matches N, plan_version's
/// VersionPlan.optional_dep_updates must contain an OptionalDepUpdate
/// entry whose manifest is the owner's canonical manifest path and
/// whose updates contains the pair (N, Y) where Y is the owner's bump
/// target.
#[test]
fn plan_version_emits_optional_dep_update_when_owner_declares_matching_optional_dep() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    std::fs::create_dir_all(root.join("crates/hybrid")).unwrap();
    // Owner's canonical manifest declares an existing optional
    // dependency on the platform member's own npm name -- Cargo
    // supports `optional = true` deps, which parse as DepKind::Optional.
    std::fs::write(
            root.join("crates/hybrid/Cargo.toml"),
            "[package]\nname = \"hybrid\"\nversion = \"1.0.0\"\n\n[dependencies]\n\"@myorg/hybrid-darwin-arm64\" = { version = \"0.9.0\", optional = true }\n",
        )
        .unwrap();
    std::fs::write(
        root.join("crates/hybrid/package.json"),
        r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.9.0","os":["darwin"],"cpu":["arm64"]}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"cargo/hybrid\", \"@myorg/hybrid-darwin-arm64\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(root.join(".changeset/bump.md"), "---\n\"hybrid\": patch\n---\n\nfix.\n").unwrap();
    commit_all(root, "add hybrid package");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    let owner_bump = plan
        .bumps
        .iter()
        .find(|b| b.package.name() == "hybrid")
        .expect("hybrid must have a planned bump");

    assert_eq!(
        plan.optional_dep_updates.len(),
        1,
        "expected exactly one OptionalDepUpdate, got: {:?}",
        plan.optional_dep_updates
    );
    let update = &plan.optional_dep_updates[0];
    assert_eq!(
        update.manifest,
        Path::new("crates/hybrid/Cargo.toml"),
        "OptionalDepUpdate.manifest must be the owner's canonical manifest path"
    );
    assert_eq!(
        update.updates,
        vec![("@myorg/hybrid-darwin-arm64".to_string(), owner_bump.to.clone())],
        "OptionalDepUpdate.updates must contain (N, Y) for the matching optional dependency"
    );
}

/// Given the owner-bumped scenario, but the owner package's
/// canonical manifest has no dependency entry (of any kind) whose name
/// matches the platform member's name N, plan_version's
/// VersionPlan.optional_dep_updates must contain no entry referencing
/// N -- an OptionalDepUpdate is only produced when a matching
/// optionalDependencies entry already exists, never speculatively
/// created.
#[test]
fn plan_version_no_optional_dep_update_when_owner_has_no_matching_dependency() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    std::fs::create_dir_all(root.join("crates/hybrid")).unwrap();
    std::fs::write(
        root.join("crates/hybrid/Cargo.toml"),
        "[package]\nname = \"hybrid\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("crates/hybrid/package.json"),
        r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.9.0","os":["darwin"],"cpu":["arm64"]}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"cargo/hybrid\", \"@myorg/hybrid-darwin-arm64\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(root.join(".changeset/bump.md"), "---\n\"hybrid\": patch\n---\n\nfix.\n").unwrap();
    commit_all(root, "add hybrid package");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    assert!(
        plan.optional_dep_updates.is_empty(),
        "optional_dep_updates must be empty when the owner's canonical manifest has no \
             matching dependency entry for the platform member's name; got: {:?}",
        plan.optional_dep_updates
    );
}

struct FixedInference {
    outcome: crate::infer::InferenceOutcome,
}

impl crate::infer::SeverityInference for FixedInference {
    fn infer(
        &self,
        _pkg: &callisto_model::Package,
        _git: &callisto_model::vcs::GitAccess<'_>,
        _window: crate::infer::InferenceWindowSpec<'_>,
    ) -> Result<Option<crate::infer::InferenceOutcome>, GraphError> {
        Ok(Some(self.outcome.clone()))
    }
}

/// Inference-driven bump with a real,
/// non-empty InferenceOutcome.commits must produce exactly one
/// ChangelogEntry with ChangeSource::Commit from commits[0] (newest,
/// since `callisto_model::vcs` shells `git log --no-merges` with no reversing
/// flag -- git's default newest-first order).
#[test]
fn plan_version_inference_reason_maps_to_most_recent_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    std::fs::create_dir_all(root.join("pkg-a")).unwrap();
    std::fs::write(
        root.join("pkg-a/Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    // release-trigger = auto: this test's FixedInference must actually run.
    std::fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"cargo/pkg-a\"\nrelease-trigger = \"auto\"\n",
    )
    .unwrap();
    commit_all(root, "add package");

    let sha_recent = callisto_model::CommitSha::parse(&"a".repeat(40)).unwrap();
    let sha_older = callisto_model::CommitSha::parse(&"b".repeat(40)).unwrap();
    let inference = FixedInference {
        outcome: crate::infer::InferenceOutcome {
            severity: callisto_model::Severity::Minor,
            commit_count: 2,
            remapped: false,
            commits: vec![
                (sha_recent.clone(), "feat: recent".to_string()),
                (sha_older, "fix: older".to_string()),
            ],
        },
    };

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    let pkg_a = callisto_model::PackageId::parse("pkg-a").unwrap();
    let write = plan
        .changelog_writes
        .iter()
        .find(|w| w.input.package == pkg_a)
        .expect("pkg-a must have a ChangelogWrite");

    assert_eq!(write.input.entries.len(), 1);
    assert_eq!(write.input.entries[0].severity, callisto_model::Severity::Minor);
    match &write.input.entries[0].source {
        crate::changelog::ChangeSource::Commit { sha, subject } => {
            assert_eq!(sha, &sha_recent);
            assert_eq!(subject, "feat: recent");
        }
        other => panic!("expected ChangeSource::Commit, got {other:?}"),
    }
}

/// An InferenceOutcome with empty `commits` but
/// nonzero `commit_count` must fall back to a ChangeSource::Changeset
/// entry summarizing the count, mirroring the changelog fallback pattern.
#[test]
fn plan_version_inference_reason_falls_back_when_commits_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    std::fs::create_dir_all(root.join("pkg-a")).unwrap();
    std::fs::write(
        root.join("pkg-a/Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    // release-trigger = auto: this test's FixedInference must actually run.
    std::fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"cargo/pkg-a\"\nrelease-trigger = \"auto\"\n",
    )
    .unwrap();
    commit_all(root, "add package");

    let inference = FixedInference {
        outcome: crate::infer::InferenceOutcome {
            severity: callisto_model::Severity::Minor,
            commit_count: 2,
            remapped: false,
            commits: vec![],
        },
    };

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    let pkg_a = callisto_model::PackageId::parse("pkg-a").unwrap();
    let write = plan
        .changelog_writes
        .iter()
        .find(|w| w.input.package == pkg_a)
        .expect("pkg-a must have a ChangelogWrite");

    assert_eq!(write.input.entries.len(), 1);
    match &write.input.entries[0].source {
        crate::changelog::ChangeSource::Changeset { filename, summary } => {
            assert_eq!(filename, "");
            assert_eq!(summary, "Inferred version bump (2 commit(s))");
        }
        other => panic!("expected ChangeSource::Changeset fallback, got {other:?}"),
    }
}

/// A package with a pending changeset AND a
/// genuine Cascade reason this run must get its changeset entries
/// followed by exactly one additional DependencyUpdate entry.
#[test]
fn plan_version_unions_changeset_with_cascade_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    std::fs::create_dir_all(root.join("pkg-core")).unwrap();
    std::fs::write(
        root.join("pkg-core/Cargo.toml"),
        "[package]\nname = \"pkg-core\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join("pkg-app")).unwrap();
    std::fs::write(
        root.join("pkg-app/Cargo.toml"),
        "[package]\nname = \"pkg-app\"\nversion = \"1.0.0\"\n\n[dependencies]\npkg-core = \"1.0.0\"\n",
    )
    .unwrap();

    std::fs::write(root.join("callisto.toml"), "[cascade]\nbump-severity = \"minor\"\n").unwrap();

    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(
        root.join(".changeset/core-break.md"),
        "---\n\"pkg-core\": major\n---\n\nBreaking API change.\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".changeset/app-patch.md"),
        "---\n\"pkg-app\": patch\n---\n\nUnrelated small fix.\n",
    )
    .unwrap();
    commit_all(root, "add packages and changesets");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");
    let inference = crate::infer::NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    let pkg_app = callisto_model::PackageId::parse("pkg-app").unwrap();
    let write = plan
        .changelog_writes
        .iter()
        .find(|w| w.input.package == pkg_app)
        .expect("pkg-app must have a ChangelogWrite");

    assert_eq!(write.input.entries.len(), 2, "got: {:?}", write.input.entries);
    assert!(matches!(
        write.input.entries[0].source,
        crate::changelog::ChangeSource::Changeset { .. }
    ));
    assert!(matches!(
        write.input.entries[1].source,
        crate::changelog::ChangeSource::DependencyUpdate { .. }
    ));
}

/// A package with
/// both a pending changeset and outcome.reasons == Some(Inference{..})
/// must get its changeset entries followed by one Inference-mapped
/// entry (Commit-or-fallback logic).
#[test]
fn plan_version_unions_changeset_with_inference_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    std::fs::create_dir_all(root.join("pkg-a")).unwrap();
    std::fs::write(
        root.join("pkg-a/Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(
        root.join(".changeset/pkg-a-patch.md"),
        "---\n\"pkg-a\": patch\n---\n\nSmall fix.\n",
    )
    .unwrap();
    // release-trigger = auto: this test's FixedInference must actually run
    // alongside the pending changeset.
    std::fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"cargo/pkg-a\"\nrelease-trigger = \"auto\"\n",
    )
    .unwrap();
    commit_all(root, "add package and changeset");

    let sha = callisto_model::CommitSha::parse(&"c".repeat(40)).unwrap();
    let inference = FixedInference {
        outcome: crate::infer::InferenceOutcome {
            severity: callisto_model::Severity::Minor,
            commit_count: 1,
            remapped: false,
            commits: vec![(sha.clone(), "feat: bigger change".to_string())],
        },
    };

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    let pkg_a = callisto_model::PackageId::parse("pkg-a").unwrap();
    let write = plan
        .changelog_writes
        .iter()
        .find(|w| w.input.package == pkg_a)
        .expect("pkg-a must have a ChangelogWrite");

    assert_eq!(write.input.entries.len(), 2, "got: {:?}", write.input.entries);
    assert!(matches!(
        write.input.entries[0].source,
        crate::changelog::ChangeSource::Changeset { .. }
    ));
    match &write.input.entries[1].source {
        crate::changelog::ChangeSource::Commit { sha: got_sha, subject } => {
            assert_eq!(got_sha, &sha);
            assert_eq!(subject, "feat: bigger change");
        }
        other => panic!("expected ChangeSource::Commit, got {other:?}"),
    }
}

/// A fixed group with one released
/// member and one fresh (never-tagged) member at the identical base
/// version; the fresh member gets a changeset-driven bump with no other
/// reason. Entries must be [Changeset, NewGroupMember] in that order.
#[test]
fn plan_version_appends_new_group_member_entry_after_changeset() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    for name in ["pkg-released", "pkg-fresh"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
        std::fs::write(
            root.join(name).join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
        )
        .unwrap();
    }
    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"grp\"\nmembers = [\"cargo/pkg-released\", \"cargo/pkg-fresh\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(
        root.join(".changeset/fresh-patch.md"),
        "---\n\"pkg-fresh\": patch\n---\n\nfix.\n",
    )
    .unwrap();
    commit_all(root, "add packages");
    tag(root, "pkg-released@1.0.0");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");
    let inference = crate::infer::NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    let pkg_fresh = callisto_model::PackageId::parse("pkg-fresh").unwrap();
    let write = plan
        .changelog_writes
        .iter()
        .find(|w| w.input.package == pkg_fresh)
        .expect("pkg-fresh must have a ChangelogWrite");

    assert_eq!(write.input.entries.len(), 2, "got: {:?}", write.input.entries);
    assert!(matches!(
        write.input.entries[0].source,
        crate::changelog::ChangeSource::Changeset { .. }
    ));
    assert!(matches!(
        write.input.entries[1].source,
        crate::changelog::ChangeSource::NewGroupMember { .. }
    ));
}

/// Regression: a `[[fixed-group]]` naming a package that has since been
/// removed from the workspace, combined with a live sibling receiving a
/// changeset-driven severity, must not crash `plan_version`. Before the
/// fix, `solve_cascade`'s Fixed-group convergence block (unlike the
/// Linked-group block right above it) wrote the stale id into
/// `CascadeOutcome.severities`/`.targets` unconditionally, and this
/// module's `pkg_map.get(id).copied().unwrap()` then panicked because
/// `pkg_map` is built only from live graph packages. The fix mirrors the
/// Linked-group guard: skip stale members and emit an `UnknownPackage`
/// diagnostic instead, matching what `aggregate::union_fixed` already
/// does for the identical scenario.
#[test]
fn plan_version_skips_stale_fixed_group_member_instead_of_panicking() {
    use crate::config::{GroupDef, GroupMember, GroupTable};
    use callisto_model::{GroupKind, GroupName, PackageId};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    std::fs::create_dir_all(root.join("pkg-live")).unwrap();
    std::fs::write(
        root.join("pkg-live").join("Cargo.toml"),
        "[package]\nname = \"pkg-live\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    // No `[[fixed-group]]` in callisto.toml -- `pkg-stale` (below) would
    // fail `GroupTable::resolve`'s `IdentityIndex` lookup at `Workspace::load`
    // time if declared there. Instead, the stale-referencing GroupTable is
    // injected directly onto the loaded workspace's config below, isolating
    // the scenario this bug is actually about: a group config (by whatever
    // means it reached this state) naming a package absent from
    // `ws.graph.packages()` / `base_versions()`.
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(
        root.join(".changeset/bump.md"),
        "---\n\"pkg-live\": patch\n---\n\nfix.\n",
    )
    .unwrap();
    commit_all(root, "add packages");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let mut ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

    let pkg_live = PackageId::parse("pkg-live").unwrap();
    let pkg_stale = PackageId::parse("pkg-stale").unwrap();
    let group_def = GroupDef {
        name: GroupName("grp".to_string()),
        kind: GroupKind::Fixed,
        members: vec![
            GroupMember::Package(pkg_live.clone()),
            GroupMember::Package(pkg_stale.clone()),
        ],
    };
    ws.config.groups = GroupTable::try_from_groups(vec![group_def], vec![]).unwrap();

    let inference = NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts)
        .expect("plan_version must gracefully skip the stale fixed-group member, not error");

    assert!(
        plan.bumps.iter().all(|b| b.package != pkg_stale),
        "the stale fixed-group member must never appear in the planned bumps: {:?}",
        plan.bumps
    );

    let unknown_diags: Vec<_> = plan
        .diagnostics
        .iter()
        .filter(|d| d.code == callisto_model::DiagnosticCode::UnknownPackage)
        .collect();
    assert!(
        !unknown_diags.is_empty(),
        "plan_version must emit an UnknownPackage diagnostic for the stale fixed-group \
             member; got diagnostics: {:?}",
        plan.diagnostics
    );
    assert!(
        unknown_diags
            .iter()
            .any(|d| d.governed_by == Some(callisto_model::ConfigKey::FIXED_GROUP)),
        "the UnknownPackage diagnostic must be governed by ConfigKey::FIXED_GROUP; got: {:?}",
        unknown_diags
    );

    assert!(
        plan.bumps.iter().any(|b| b.package == pkg_live),
        "the live sibling pkg-live must still be bumped normally: {:?}",
        plan.bumps
    );
}

/// Same fixed-group
/// fixture, but pkg-fresh is ALSO a cascade target this run (via a
/// third driver package), so it has both a changeset entry and a
/// cascade-mapped entry (T4) before the NewGroupMember append.
#[test]
fn plan_version_new_group_member_entry_is_additive_after_cascade_union() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git_init_with_commit(root);

    for name in ["pkg-released", "pkg-fresh"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
        std::fs::write(
            root.join(name).join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
        )
        .unwrap();
    }
    std::fs::create_dir_all(root.join("pkg-driver")).unwrap();
    std::fs::write(
        root.join("pkg-driver/Cargo.toml"),
        "[package]\nname = \"pkg-driver\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("pkg-fresh/Cargo.toml"),
        "[package]\nname = \"pkg-fresh\"\nversion = \"1.0.0\"\n\n[dependencies]\npkg-driver = \"1.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"grp\"\nmembers = [\"cargo/pkg-released\", \"cargo/pkg-fresh\"]\n\n[cascade]\nbump-severity = \"minor\"\n",
        )
        .unwrap();
    std::fs::create_dir_all(root.join(".changeset")).unwrap();
    std::fs::write(
        root.join(".changeset/fresh-patch.md"),
        "---\n\"pkg-fresh\": patch\n---\n\nfix.\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".changeset/driver-major.md"),
        "---\n\"pkg-driver\": major\n---\n\nBreaking.\n",
    )
    .unwrap();
    commit_all(root, "add packages");
    tag(root, "pkg-released@1.0.0");

    let locator = IgnoreWalkLocator::new(root);
    let runner = GitRunner;
    let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");
    let inference = crate::infer::NoInference;
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };

    let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

    let pkg_fresh = callisto_model::PackageId::parse("pkg-fresh").unwrap();
    let write = plan
        .changelog_writes
        .iter()
        .find(|w| w.input.package == pkg_fresh)
        .expect("pkg-fresh must have a ChangelogWrite");

    assert_eq!(write.input.entries.len(), 3, "got: {:?}", write.input.entries);
    assert!(matches!(
        write.input.entries[0].source,
        crate::changelog::ChangeSource::Changeset { .. }
    ));
    assert!(matches!(
        write.input.entries[1].source,
        crate::changelog::ChangeSource::DependencyUpdate { .. }
    ));
    assert!(matches!(
        write.input.entries[2].source,
        crate::changelog::ChangeSource::NewGroupMember { .. }
    ));
}
