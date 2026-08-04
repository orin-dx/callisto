use callisto_model::ApplyPermit;
mod fixtures;
use callisto_graph::cascade::{run_cascade, CascadeInput};
use callisto_graph::config::groups::GroupTable;
use callisto_graph::config::{CascadeBumpSeverity, CascadeConfig, CascadeMode};
use callisto_model::{DepKind, DepSpec, PackageId, Severity, Version, VersionReq};
use fixtures::GraphBuilder;
use std::cell::OnceCell;
use std::collections::BTreeMap;

#[test]
fn test_blackbox_cascade_propagation() {
    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();

    let graph = GraphBuilder::new()
        .package(pkg_a.clone(), |p| p)
        .package(pkg_b.clone(), |p| p)
        .edge(
            pkg_a.clone(),
            pkg_b.clone(),
            DepKind::Runtime,
            DepSpec::Range(
                VersionReq::parse("^1.0.0", callisto_model::Ecosystem::Cargo).unwrap(),
                "^1.0.0".to_string(),
            ),
        )
        .build()
        .unwrap();

    let groups = GroupTable::default();
    let cfg = CascadeConfig {
        mode: CascadeMode::OutOfRange,
        bump_severity: CascadeBumpSeverity::Patch,
        peer_escalation: true,
        preserve_npm_ranges: false,
    };

    let mut seed = BTreeMap::new();
    seed.insert(pkg_b.clone(), Severity::Major);

    let mut base = BTreeMap::new();
    base.insert(pkg_a.clone(), Version::semver(1, 0, 0));
    base.insert(pkg_b.clone(), Version::semver(1, 0, 0));

    let reasons = BTreeMap::new();
    let named_by = BTreeMap::new();

    let input = CascadeInput {
        graph: &graph,
        groups: &groups,
        cfg: &cfg,
        seed: &seed,
        reasons: &reasons,
        named_by: &named_by,
        base: &base,
        pre: None,
    };

    let outcome = run_cascade(input).unwrap();
    assert_eq!(outcome.severities.get(&pkg_b), Some(&Severity::Major));
}

#[test]
fn test_diamond_cascade_convergence() {
    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();
    let pkg_c = PackageId::parse("pkg-c").unwrap();
    let pkg_d = PackageId::parse("pkg-d").unwrap();

    let graph = GraphBuilder::new()
        .package(pkg_a.clone(), |p| p)
        .package(pkg_b.clone(), |p| p)
        .package(pkg_c.clone(), |p| p)
        .package(pkg_d.clone(), |p| p)
        .edge(
            pkg_a.clone(),
            pkg_b.clone(),
            DepKind::Runtime,
            DepSpec::Range(
                VersionReq::parse("^1.0.0", callisto_model::Ecosystem::Cargo).unwrap(),
                "^1.0.0".to_string(),
            ),
        )
        .edge(
            pkg_a.clone(),
            pkg_c.clone(),
            DepKind::Runtime,
            DepSpec::Range(
                VersionReq::parse("^1.0.0", callisto_model::Ecosystem::Cargo).unwrap(),
                "^1.0.0".to_string(),
            ),
        )
        .edge(
            pkg_b.clone(),
            pkg_d.clone(),
            DepKind::Runtime,
            DepSpec::Range(
                VersionReq::parse("^1.0.0", callisto_model::Ecosystem::Cargo).unwrap(),
                "^1.0.0".to_string(),
            ),
        )
        .edge(
            pkg_c.clone(),
            pkg_d.clone(),
            DepKind::Runtime,
            DepSpec::Range(
                VersionReq::parse("^1.0.0", callisto_model::Ecosystem::Cargo).unwrap(),
                "^1.0.0".to_string(),
            ),
        )
        .build()
        .unwrap();

    let groups = GroupTable::default();
    let cfg = CascadeConfig {
        mode: CascadeMode::OutOfRange,
        bump_severity: CascadeBumpSeverity::Patch,
        peer_escalation: true,
        preserve_npm_ranges: false,
    };

    let mut seed = BTreeMap::new();
    seed.insert(pkg_d.clone(), Severity::Major);

    let mut base = BTreeMap::new();
    base.insert(pkg_a.clone(), Version::semver(1, 0, 0));
    base.insert(pkg_b.clone(), Version::semver(1, 0, 0));
    base.insert(pkg_c.clone(), Version::semver(1, 0, 0));
    base.insert(pkg_d.clone(), Version::semver(1, 0, 0));

    let reasons = BTreeMap::new();
    let named_by = BTreeMap::new();

    let input = CascadeInput {
        graph: &graph,
        groups: &groups,
        cfg: &cfg,
        seed: &seed,
        reasons: &reasons,
        named_by: &named_by,
        base: &base,
        pre: None,
    };

    let outcome = run_cascade(input).unwrap();
    assert_eq!(outcome.severities.get(&pkg_d), Some(&Severity::Major));
    assert!(outcome.severities.get(&pkg_b).unwrap() >= &Severity::Patch);
    assert!(outcome.severities.get(&pkg_c).unwrap() >= &Severity::Patch);
}

#[test]
fn test_peer_dependency_escalation() {
    let pkg_app = PackageId::parse("pkg-app").unwrap();
    let pkg_plugin = PackageId::parse("pkg-plugin").unwrap();

    let graph = GraphBuilder::new()
        .package(pkg_app.clone(), |p| p)
        .package(pkg_plugin.clone(), |p| p)
        .edge(
            pkg_plugin.clone(),
            pkg_app.clone(),
            DepKind::Peer,
            DepSpec::Range(
                VersionReq::parse("^1.0.0", callisto_model::Ecosystem::Npm).unwrap(),
                "^1.0.0".to_string(),
            ),
        )
        .build()
        .unwrap();

    let groups = GroupTable::default();
    let cfg = CascadeConfig {
        mode: CascadeMode::OutOfRange,
        bump_severity: CascadeBumpSeverity::Patch,
        peer_escalation: true,
        preserve_npm_ranges: false,
    };

    let mut seed = BTreeMap::new();
    seed.insert(pkg_app.clone(), Severity::Major);

    let mut base = BTreeMap::new();
    base.insert(pkg_app.clone(), Version::semver(1, 0, 0));
    base.insert(pkg_plugin.clone(), Version::semver(1, 0, 0));

    let reasons = BTreeMap::new();
    let named_by = BTreeMap::new();

    let input = CascadeInput {
        graph: &graph,
        groups: &groups,
        cfg: &cfg,
        seed: &seed,
        reasons: &reasons,
        named_by: &named_by,
        base: &base,
        pre: None,
    };

    let outcome = run_cascade(input).unwrap();
    assert_eq!(outcome.severities.get(&pkg_plugin), Some(&Severity::Major));
}

#[test]
fn test_linked_group_converges_shared_version() {
    use callisto_graph::config::{GroupDef, GroupMember};
    use callisto_model::{GroupKind, GroupName};

    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();

    let graph = GraphBuilder::new()
        .package(pkg_a.clone(), |p| p)
        .package(pkg_b.clone(), |p| p)
        .build()
        .unwrap();

    let mut groups = GroupTable::default();
    let group_name = GroupName("group-ab".to_string());
    groups.linked.insert(
        group_name.clone(),
        GroupDef {
            name: group_name.clone(),
            kind: GroupKind::Linked,
            members: vec![
                GroupMember::Package(pkg_a.clone()),
                GroupMember::Package(pkg_b.clone()),
            ],
        },
    );
    groups.linked_of.insert(pkg_a.clone(), group_name.clone());
    groups.linked_of.insert(pkg_b.clone(), group_name);

    let cfg = CascadeConfig {
        mode: CascadeMode::OutOfRange,
        bump_severity: CascadeBumpSeverity::Patch,
        peer_escalation: true,
        preserve_npm_ranges: false,
    };

    let mut seed = BTreeMap::new();
    seed.insert(pkg_a.clone(), Severity::Patch);

    let mut base = BTreeMap::new();
    base.insert(pkg_a.clone(), Version::semver(1, 0, 0));
    base.insert(pkg_b.clone(), Version::semver(2, 0, 0));

    let reasons = BTreeMap::new();
    let named_by = BTreeMap::new();

    let input = CascadeInput {
        graph: &graph,
        groups: &groups,
        cfg: &cfg,
        seed: &seed,
        reasons: &reasons,
        named_by: &named_by,
        base: &base,
        pre: None,
    };

    let outcome = run_cascade(input).unwrap();

    // Spec §G.6.7: Linked group syncs release severities across members, and
    // members converge on a single winning target version (the max of each
    // member's individually-computed candidate) rather than diverging by
    // their own base version: pkg-a's candidate is 1.0.1, pkg-b's is 2.0.1,
    // so both converge on the winner, 2.0.1.
    assert_eq!(outcome.severities.get(&pkg_a), Some(&Severity::Patch));
    assert_eq!(outcome.severities.get(&pkg_b), Some(&Severity::Patch));
    assert_eq!(outcome.targets.get(&pkg_a), Some(&Version::semver(2, 0, 1)));
    assert_eq!(outcome.targets.get(&pkg_b), Some(&Version::semver(2, 0, 1)));
}

#[test]
fn test_absolute_path_workspace_cargo_resolver() {
    let temp_dir = tempfile::tempdir().unwrap();
    let root_cargo = temp_dir.path().join("Cargo.toml");
    let content = r#"[workspace]
members = ["crates/sub"]
resolver = "2"

[workspace.package]
version = "0.2.0"
"#;
    std::fs::write(&root_cargo, content).unwrap();

    // Must load without error when passed an absolute path
    let resolver = callisto_manifests::WorkspaceCargoResolver::load(&root_cargo);
    assert!(resolver.is_ok());

    let inh = resolver.unwrap().inheritance().unwrap();
    assert_eq!(inh.version.unwrap().render(), "0.2.0");
}

#[test]
fn test_leading_dash_package_id_rejection() {
    let result = PackageId::parse("-x");
    assert!(result.is_err());
}

#[test]
fn test_atomic_write_utility() {
    let temp_dir = tempfile::tempdir().unwrap();
    let target_file = temp_dir.path().join("atomic_test.txt");
    let content = "callisto atomic write test payload\n";

    callisto_manifests::atomic::atomic_write(
        &target_file,
        content,
        &ApplyPermit::force_for_tests(),
    )
    .unwrap();

    assert!(target_file.exists());
    let read_back = std::fs::read_to_string(&target_file).unwrap();
    assert_eq!(read_back, content);
}

#[test]
fn test_validate_detects_empty_changesets() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cs_dir = temp_dir.path().join(".changeset");
    std::fs::create_dir_all(&cs_dir).unwrap();
    std::fs::write(cs_dir.join("empty.md"), "---\n---\n").unwrap();

    let cfg = callisto_graph::config::load(&temp_dir.path().join("callisto.toml")).unwrap();
    let loaded = callisto_graph::load_changesets(temp_dir.path(), &cfg);
    assert!(loaded.is_err());
}

#[test]
fn test_linked_group_version_convergence() {
    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();

    let graph = GraphBuilder::new()
        .package(pkg_a.clone(), |p| p)
        .package(pkg_b.clone(), |p| p)
        .build()
        .unwrap();

    let mut base = BTreeMap::new();
    base.insert(pkg_a.clone(), Version::semver(1, 4, 0));
    base.insert(pkg_b.clone(), Version::semver(2, 7, 3));

    let mut initial_severities = BTreeMap::new();
    initial_severities.insert(pkg_a.clone(), Severity::Minor);
    initial_severities.insert(pkg_b.clone(), Severity::Minor);

    let mut groups = GroupTable::default();
    let mut group_def = callisto_graph::config::GroupDef {
        name: callisto_model::GroupName("core_linked".to_string()),
        kind: callisto_model::GroupKind::Linked,
        members: Vec::new(),
    };
    group_def
        .members
        .push(callisto_graph::config::GroupMember::Package(pkg_a.clone()));
    group_def
        .members
        .push(callisto_graph::config::GroupMember::Package(pkg_b.clone()));
    groups.linked.insert(group_def.name.clone(), group_def);

    let cfg = CascadeConfig {
        mode: CascadeMode::OutOfRange,
        bump_severity: CascadeBumpSeverity::Patch,
        peer_escalation: true,
        preserve_npm_ranges: false,
    };
    let named_by = BTreeMap::new();
    let reasons = BTreeMap::new();

    let input = CascadeInput {
        graph: &graph,
        groups: &groups,
        cfg: &cfg,
        seed: &initial_severities,
        reasons: &reasons,
        named_by: &named_by,
        base: &base,
        pre: None,
    };

    let outcome = run_cascade(input).unwrap();
    let target_a = outcome.targets.get(&pkg_a).unwrap().render();
    let target_b = outcome.targets.get(&pkg_b).unwrap().render();

    // Spec §G.6.7: Linked group members sync release severity (Minor) AND
    // converge on the single winning target version: pkg_a's candidate is
    // 1.5.0, pkg_b's is 2.8.0, so both converge on the winner, 2.8.0.
    assert_eq!(target_a, "2.8.0");
    assert_eq!(target_b, "2.8.0");
    assert_eq!(outcome.severities.get(&pkg_a), Some(&Severity::Minor));
    assert_eq!(outcome.severities.get(&pkg_b), Some(&Severity::Minor));
}

#[test]
fn test_tag_dry_run_does_not_create_git_tags() {
    use callisto_graph::commands::TagOptions;
    use callisto_model::{
        CommandOutput, CommitSha, PackageId, PublishPlan, ReleaseEntry, TagName, SCHEMA_VERSION,
    };

    let pkg_id = PackageId::parse("callisto-cli").unwrap();
    let tag_name = TagName("callisto-cli@0.2.0".to_string());
    let sha = CommitSha::parse("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0").unwrap();

    let plan = PublishPlan {
        schema_version: SCHEMA_VERSION,
        rust_crates: Vec::new(),
        npm_main_packages: Vec::new(),
        npm_platform_packages: Vec::new(),
        pypi_packages: Vec::new(),
        releases: vec![ReleaseEntry {
            package: pkg_id.clone(),
            tag_name: tag_name.clone(),
            sha,
            changelog_section: Some("Initial release".to_string()),
        }],
        diagnostics: Vec::new(),
    };

    struct DryRunTestRunner;
    impl callisto_model::CommandRunner for DryRunTestRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<CommandOutput, callisto_model::CommandError> {
            if program == "git" && args.first() == Some(&"tag") && args.get(1) == Some(&"-a") {
                panic!("git tag -a MUST NOT be called when dry_run is true");
            }
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: "".to_string(),
                stderr: "".to_string(),
            })
        }
    }

    let runner = DryRunTestRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    let cfg = callisto_graph::config::load(&ws_dir.path().join("callisto.toml")).unwrap();
    let graph = GraphBuilder::new().package(pkg_id, |p| p).build().unwrap();
    std::process::Command::new("git")
        .arg("init")
        .current_dir(ws_dir.path())
        .output()
        .unwrap();
    let tags = callisto_graph::tags::TagIndex::build(&runner, ws_dir.path(), &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: ws_dir.path().to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        runner: &runner,
        manifest_cache: Default::default(),
    };

    let opts = TagOptions {
        floating_major: false,
    };
    // `None` is the dry run: the report still lists the tag that would be
    // created, and no git ref is touched.
    let report =
        callisto_graph::commands::create_tags_with_options(&ws, &plan, &opts, None).unwrap();
    assert_eq!(report.created_tags.len(), 1);
    assert_eq!(
        report.created_tags[0].tag_name.as_str(),
        "callisto-cli@0.2.0"
    );
}

/// Spec: `create_tags_with_options` must not hard-crash via
/// `GitRepository::discover(&ws.root)?` when gix is unavailable -- exactly
/// the situation `wasm32` is permanently in, since gix is excluded from that
/// target's dependency set. It must fall back to the `CommandRunner`,
/// mirroring the gix-try/`CommandRunner`-fallback shape `tags.rs`'s
/// `fetch_all_tags` already uses for the read-only case, extended to also
/// cover the two mutating operations this command performs: checking
/// whether a tag already exists, and creating the annotated release tag
/// plus (when requested) force-moving the floating major tag.
#[test]
fn test_create_tags_without_gix_falls_back_to_command_runner() {
    use callisto_graph::commands::TagOptions;
    use callisto_model::{
        CommandOutput, CommitSha, PackageId, PublishPlan, ReleaseEntry, TagName, SCHEMA_VERSION,
    };
    use std::sync::Mutex;

    let pkg_id = PackageId::parse("callisto-cli").unwrap();
    let tag_name = TagName("callisto-cli@1.0.0".to_string());
    let sha = CommitSha::parse("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0").unwrap();

    let plan = PublishPlan {
        schema_version: SCHEMA_VERSION,
        rust_crates: Vec::new(),
        npm_main_packages: Vec::new(),
        npm_platform_packages: Vec::new(),
        pypi_packages: Vec::new(),
        releases: vec![ReleaseEntry {
            package: pkg_id.clone(),
            tag_name: tag_name.clone(),
            sha: sha.clone(),
            changelog_section: Some("Initial release".to_string()),
        }],
        diagnostics: Vec::new(),
    };

    /// Records every `git` invocation (as owned arg vectors, so assertions
    /// can inspect them after the run) and answers the way a real `git`
    /// binary would against an empty, tagless repo -- `tag --list <name>`
    /// with no matches, everything else with success -- without ever
    /// touching gix.
    struct NoGixTestRunner {
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl callisto_model::CommandRunner for NoGixTestRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<CommandOutput, callisto_model::CommandError> {
            assert_eq!(program, "git");
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|s| s.to_string()).collect());
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    let runner = NoGixTestRunner {
        calls: Mutex::new(Vec::new()),
    };

    // Deliberately NOT `git init`-ed: `GitRepository::discover` fails here
    // exactly the way it unconditionally does on wasm32, so this is the
    // native-testable stand-in for "gix is unavailable" that forces every
    // path under test through the `CommandRunner` fallback.
    let ws_dir = tempfile::tempdir().unwrap();
    assert!(
        callisto_vcs::GitRepository::discover(ws_dir.path()).is_err(),
        "test fixture must not be discoverable as a Git repo"
    );

    let cfg = callisto_graph::config::load(ws_dir.path()).unwrap();
    let graph = GraphBuilder::new()
        .package(pkg_id.clone(), |p| p)
        .build()
        .unwrap();
    let tags = callisto_graph::tags::TagIndex::build(&runner, ws_dir.path(), &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: ws_dir.path().to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        runner: &runner,
        manifest_cache: Default::default(),
    };

    let opts = TagOptions {
        floating_major: true,
    };

    let report = callisto_graph::commands::create_tags_with_options(
        &ws,
        &plan,
        &opts,
        Some(&ApplyPermit::force_for_tests()),
    )
    .expect(
        "create_tags_with_options must not hard-crash via GitRepository::discover's `?` \
             when gix is unavailable -- it must fall back to the CommandRunner, mirroring \
             tags.rs's fetch_all_tags",
    );

    let created: Vec<&str> = report
        .created_tags
        .iter()
        .map(|t| t.tag_name.as_str())
        .collect();
    assert!(created.contains(&"callisto-cli@1.0.0"));
    assert!(created.contains(&"callisto-cli@1"));

    let calls = runner.calls.lock().unwrap();

    // Must have shelled out to check whether the release tag already
    // exists. `GitDataSource::list_tags`'s shell backend always fetches the
    // *unfiltered* tag list (`git tag --list`, no glob argument) and
    // applies `globset` matching locally, rather than delegating to `git
    // tag --list <pattern>`'s own (different) glob dialect -- this is what
    // guarantees byte-identical tag selection between the native gix path
    // and this fallback, so the shelled call carries no tag-name argument.
    let expected_list_call: Vec<String> = vec!["tag".into(), "--list".into()];
    assert!(
        calls.contains(&expected_list_call),
        "expected a `git tag --list` call, got: {calls:?}"
    );

    // ...to create the annotated release tag itself, matching
    // `GitRepository::create_tag`'s current annotated-tag semantics
    // (`Some(message)` branch: `tag -a <name> -m <message> <sha>`)...
    let expected_create_call: Vec<String> = vec![
        "tag".into(),
        "-a".into(),
        "callisto-cli@1.0.0".into(),
        "-m".into(),
        "Release callisto-cli@1.0.0".into(),
        sha.as_str().to_string(),
    ];
    assert!(
        calls.contains(&expected_create_call),
        "expected an annotated `git tag -a` create call, got: {calls:?}"
    );

    // ...and to force-move the floating major tag, matching
    // `GitRepository::create_floating_major`'s unconditional-overwrite
    // (`PreviousValue::Any`) semantics.
    let expected_floating_call: Vec<String> = vec![
        "tag".into(),
        "-f".into(),
        "callisto-cli@1".into(),
        sha.as_str().to_string(),
    ];
    assert!(
        calls.contains(&expected_floating_call),
        "expected a force `git tag -f` call for the floating major tag, got: {calls:?}"
    );
}

/// Spec: when the `CommandRunner` fallback reports the release tag already
/// exists (`git tag --list <name>` returns a match), `create_tags_with_options`
/// must not attempt to create it again -- mirroring the `already_existed`
/// gate the gix path already applies via `repo.list_tags`/`repo.create_tag`.
#[test]
fn test_create_tags_without_gix_skips_creation_for_existing_tag() {
    use callisto_graph::commands::TagOptions;
    use callisto_model::{
        CommandOutput, CommitSha, PackageId, PublishPlan, ReleaseEntry, TagName, SCHEMA_VERSION,
    };
    use std::sync::Mutex;

    let pkg_id = PackageId::parse("callisto-cli").unwrap();
    let tag_name = TagName("callisto-cli@1.0.0".to_string());
    let sha = CommitSha::parse("a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0").unwrap();

    let plan = PublishPlan {
        schema_version: SCHEMA_VERSION,
        rust_crates: Vec::new(),
        npm_main_packages: Vec::new(),
        npm_platform_packages: Vec::new(),
        pypi_packages: Vec::new(),
        releases: vec![ReleaseEntry {
            package: pkg_id.clone(),
            tag_name: tag_name.clone(),
            sha: sha.clone(),
            changelog_section: None,
        }],
        diagnostics: Vec::new(),
    };

    struct AlreadyTaggedTestRunner {
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl callisto_model::CommandRunner for AlreadyTaggedTestRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<CommandOutput, callisto_model::CommandError> {
            assert_eq!(program, "git");
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|s| s.to_string()).collect());
            let stdout = if args.first() == Some(&"tag") && args.get(1) == Some(&"--list") {
                "callisto-cli@1.0.0".to_string()
            } else {
                String::new()
            };
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout,
                stderr: String::new(),
            })
        }
    }

    let runner = AlreadyTaggedTestRunner {
        calls: Mutex::new(Vec::new()),
    };

    let ws_dir = tempfile::tempdir().unwrap();
    assert!(
        callisto_vcs::GitRepository::discover(ws_dir.path()).is_err(),
        "test fixture must not be discoverable as a Git repo"
    );

    let cfg = callisto_graph::config::load(ws_dir.path()).unwrap();
    let graph = GraphBuilder::new()
        .package(pkg_id.clone(), |p| p)
        .build()
        .unwrap();
    let tags = callisto_graph::tags::TagIndex::build(&runner, ws_dir.path(), &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: ws_dir.path().to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        runner: &runner,
        manifest_cache: Default::default(),
    };

    let opts = TagOptions {
        floating_major: false,
    };

    let report = callisto_graph::commands::create_tags_with_options(
        &ws,
        &plan,
        &opts,
        Some(&ApplyPermit::force_for_tests()),
    )
    .expect("create_tags_with_options must succeed via the CommandRunner fallback");
    assert_eq!(report.created_tags.len(), 1);

    let calls = runner.calls.lock().unwrap();
    assert!(
        !calls
            .iter()
            .any(|c| c.first().map(String::as_str) == Some("tag")
                && c.get(1).map(String::as_str) == Some("-a")),
        "must not attempt to re-create a tag the fallback reported as already existing, got: {calls:?}"
    );
}

#[test]
fn test_validate_since_git_diff_argument_ordering() {
    use callisto_graph::commands::{validate, ValidateOptions};
    use callisto_model::{CommandOutput, CommandRunner};
    use std::sync::atomic::{AtomicBool, Ordering};

    static CALLED_CORRECTLY: AtomicBool = AtomicBool::new(false);

    struct ValidateArgTestRunner;
    impl CommandRunner for ValidateArgTestRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<CommandOutput, callisto_model::CommandError> {
            if program == "git"
                && args.len() >= 4
                && args[0] == "diff"
                && args[1] == "--name-only"
                && args[2] == "main..HEAD"
                && args[3] == "--"
            {
                CALLED_CORRECTLY.store(true, Ordering::SeqCst);
            }
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: "".to_string(),
                stderr: "".to_string(),
            })
        }
    }

    let runner = ValidateArgTestRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    let cfg = callisto_graph::config::load(&ws_dir.path().join("callisto.toml")).unwrap();
    let graph = GraphBuilder::new().build().unwrap();
    let tags = callisto_graph::tags::TagIndex::build(&runner, ws_dir.path(), &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: ws_dir.path().to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        runner: &runner,
        manifest_cache: Default::default(),
    };

    let opts = ValidateOptions {
        staged: false,
        since: Some("main".to_string()),
        strict: false,
        strict_graph: false,
    };

    let _res = validate(&ws, &opts);
    assert!(
        CALLED_CORRECTLY.load(Ordering::SeqCst),
        "git diff argument ordering must be `git diff --name-only main..HEAD --`"
    );
}

/// Runs `git` for test-fixture setup only (not exercised through `CommandRunner`,
/// since `plan_snapshot`'s HEAD sha resolution goes through `callisto_vcs::GitRepository`,
/// which talks to a real on-disk repo directly).
fn run_git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git should run");
    assert!(status.success(), "git {args:?} failed");
}

/// Initializes a real Git repo at `dir` with one commit and returns the full 40-char HEAD sha.
fn init_git_repo_with_commit(dir: &std::path::Path) -> String {
    run_git(dir, &["init", "-q"]);
    run_git(dir, &["config", "user.email", "test@example.com"]);
    run_git(dir, &["config", "user.name", "Test"]);
    run_git(dir, &["add", "."]);
    run_git(dir, &["commit", "-q", "-m", "init"]);

    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .expect("git rev-parse HEAD should run");
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

struct DummyRunner;
impl callisto_model::CommandRunner for DummyRunner {
    fn run(
        &self,
        _program: &str,
        _args: &[&str],
        _cwd: &std::path::Path,
    ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
        Ok(callisto_model::CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

#[test]
fn test_snapshot_version_template_placeholders() {
    use callisto_graph::commands::plan_snapshot;

    let runner = DummyRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    // `plan_snapshot` must resolve a real HEAD sha (§G.11), so the fixture needs a
    // real Git repository with at least one commit, not just a bare temp dir.
    std::fs::write(root.join(".gitkeep"), "").unwrap();
    let head_sha = init_git_repo_with_commit(root);
    let expected_sha7 = &head_sha[..7];

    let cfg = callisto_graph::config::load(&root.join("callisto.toml")).unwrap();
    let graph = GraphBuilder::new().build().unwrap();
    let tags = callisto_graph::tags::TagIndex::build(&runner, root, &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        runner: &runner,
        manifest_cache: Default::default(),
    };

    let (_plan, report) = plan_snapshot(&ws, "canary").unwrap();
    assert_eq!(report.schema_version, callisto_model::SCHEMA_VERSION);
    assert_eq!(report.snapshot_tag, format!("0.0.0-canary-{expected_sha7}"));
}

/// docs/01-spec.md §G.11 (SPEC DECISION, pinned invariant #33): the snapshot version is
/// exactly `0.0.0-{tag}-{sha7}` — base literally `0.0.0` (never the package's own version),
/// hyphen-joined (never dot-joined), and **identical for every package in the workspace**.
/// This is what makes a snapshot unpublishable-over-a-real-release: every genuine release
/// version sorts above `0.0.0-...` in SemVer precedence.
#[test]
fn test_snapshot_version_format_matches_spec() {
    use callisto_graph::commands::plan_snapshot;

    let runner = DummyRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    // Two packages with distinct, non-trivial real versions: if the implementation
    // (bug) bases the snapshot version on the package's own version, these two
    // packages will disagree; per spec, they must produce the identical string.
    let pkg_a_dir = root.join("pkg-a");
    std::fs::create_dir_all(&pkg_a_dir).unwrap();
    std::fs::write(
        pkg_a_dir.join("Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.4.2\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let pkg_b_dir = root.join("pkg-b");
    std::fs::create_dir_all(&pkg_b_dir).unwrap();
    std::fs::write(
        pkg_b_dir.join("Cargo.toml"),
        "[package]\nname = \"pkg-b\"\nversion = \"2.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let head_sha = init_git_repo_with_commit(root);
    let expected_sha7 = &head_sha[..7];
    let expected_version = format!("0.0.0-canary-{expected_sha7}");

    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();
    let graph = GraphBuilder::new()
        .package(pkg_a.clone(), |p| p)
        .package(pkg_b.clone(), |p| p)
        .build()
        .unwrap();

    let cfg = callisto_graph::config::load(&root.join("callisto.toml")).unwrap();
    let tags = callisto_graph::tags::TagIndex::build(&runner, root, &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        runner: &runner,
        manifest_cache: Default::default(),
    };

    let (plan, report) =
        plan_snapshot(&ws, "canary").expect("plan_snapshot should succeed against a real repo");

    assert_eq!(
        report.snapshot_tag, expected_version,
        "snapshot_tag must be exactly `0.0.0-{{tag}}-{{sha7}}` per docs/01-spec.md §G.11"
    );

    assert_eq!(plan.bumps.len(), 2, "expected one planned bump per package");
    for bump in &plan.bumps {
        assert_eq!(
            bump.to.render(),
            expected_version.as_str(),
            "package `{}` must receive the identical workspace-wide snapshot version, \
             not a version derived from its own current version (§G.11 invariant #33)",
            bump.package.display_name()
        );
    }

    assert_eq!(report.bumps.len(), 2);
    for bump in &report.bumps {
        assert_eq!(bump.to.render(), expected_version.as_str());
    }
}

/// docs/01-spec.md §G.11: the snapshot sha is `CommitSha::short()` of a resolved HEAD.
/// When HEAD cannot be resolved (no repo, no commits, etc.), `plan_snapshot` must return a
/// real, surfaced error — not silently substitute a fake `0000000` placeholder sha, which
/// would let snapshots from unrelated runs collide on the same tag.
#[test]
fn test_snapshot_sha_resolution_failure_is_surfaced_error() {
    use callisto_graph::commands::plan_snapshot;

    let runner = DummyRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    // Deliberately no `git init`: the workspace root is not part of any Git repository,
    // so HEAD sha resolution must fail.
    let cfg = callisto_graph::config::load(&ws_dir.path().join("callisto.toml")).unwrap();
    let graph = GraphBuilder::new().build().unwrap();
    let tags = callisto_graph::tags::TagIndex::build(&runner, ws_dir.path(), &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: ws_dir.path().to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        runner: &runner,
        manifest_cache: Default::default(),
    };

    let result = plan_snapshot(&ws, "canary");

    assert!(
        result.is_err(),
        "plan_snapshot must surface a real error when HEAD sha cannot be resolved, \
         not silently succeed with a fake `0000000` placeholder sha"
    );
    assert!(
        matches!(result, Err(callisto_graph::GraphError::Vcs(_))),
        "expected a GraphError::Vcs from failed sha discovery, got: {result:?}"
    );
}

#[test]
fn test_compose_pr_body_custom_branch_flag() {
    use callisto_graph::commands::{compose_pr_body, PrBodyOptions};
    use callisto_graph::infer::NoInference;

    struct DummyRunner;
    impl callisto_model::CommandRunner for DummyRunner {
        fn run(
            &self,
            _program: &str,
            _args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            Ok(callisto_model::CommandOutput {
                exit_code: Some(0),
                stdout: "".to_string(),
                stderr: "".to_string(),
            })
        }
    }

    let runner = DummyRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    let cfg = callisto_graph::config::load(&ws_dir.path().join("callisto.toml")).unwrap();
    let graph = GraphBuilder::new().build().unwrap();
    let tags = callisto_graph::tags::TagIndex::build(&runner, ws_dir.path(), &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: ws_dir.path().to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        runner: &runner,
        manifest_cache: Default::default(),
    };

    let inference = NoInference;
    let opts = PrBodyOptions {
        existing_body: None,
        labels: Vec::new(),
        branch: Some("release/v1.0".to_string()),
    };

    let report = compose_pr_body(&ws, &inference, &opts).unwrap();
    assert!(report.pr_body.contains("release/v1.0"));
}

#[test]
fn test_apply_version_plan_reports_git_add_failure() {
    use callisto_graph::apply::{apply_version_plan, ApplyOptions};
    use callisto_graph::plan::VersionPlan;
    use callisto_model::{CommandOutput, CommandRunner};

    // Runner whose `git add` invocation always fails with a non-zero exit
    // code (e.g. pathspec rejected by a hook, disk full, etc.) while still
    // returning `Ok` at the process-spawn level.
    struct FailingGitAddRunner;
    impl CommandRunner for FailingGitAddRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<CommandOutput, callisto_model::CommandError> {
            if program == "git" && args.first() == Some(&"add") {
                return Ok(CommandOutput {
                    exit_code: Some(1),
                    stdout: String::new(),
                    stderr: "fatal: pathspec rejected by hook".to_string(),
                });
            }
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    let runner = FailingGitAddRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .arg("init")
        .current_dir(ws_dir.path())
        .output()
        .unwrap();

    // A plan that writes a `.changeset/pre.json` via `pre_state_update`,
    // which apply_version_plan will pick up as a modified path that still
    // exists on disk (routed through the `git add --` branch).
    let pre_state = callisto_format::PreState::entering("canary", Vec::new());
    let plan = VersionPlan {
        pre_state_update: Some(pre_state),
        ..Default::default()
    };

    let opts = ApplyOptions {
        refresh_lockfiles: false,
    };

    let result = apply_version_plan(
        ws_dir.path(),
        &plan,
        &runner,
        &opts,
        &ApplyPermit::force_for_tests(),
    );

    // The manifest write itself succeeded (pre.json was written to disk),
    // but `git add` failed. apply_version_plan must surface that failure
    // rather than silently reporting the path as staged.
    assert!(
        result.is_err(),
        "expected apply_version_plan to return an error when `git add` fails, got: {result:?}"
    );
}
