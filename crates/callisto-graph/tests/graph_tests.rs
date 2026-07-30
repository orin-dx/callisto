mod fixtures;
use callisto_graph::cascade::{run_cascade, CascadeInput};
use callisto_graph::config::groups::GroupTable;
use callisto_graph::config::{CascadeBumpSeverity, CascadeConfig, CascadeMode};
use callisto_model::{DepKind, DepSpec, PackageId, Severity, Version, VersionReq};
use fixtures::GraphBuilder;
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
fn test_linked_group_maintains_independent_versions() {
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

    // Spec §G.6.7: Linked group syncs release severities across members,
    // so both packages receive Patch bump, yielding 1.0.1 for pkg-a and 2.0.1 for pkg-b.
    assert_eq!(outcome.severities.get(&pkg_a), Some(&Severity::Patch));
    assert_eq!(outcome.severities.get(&pkg_b), Some(&Severity::Patch));
    assert_eq!(outcome.targets.get(&pkg_a), Some(&Version::semver(1, 0, 1)));
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

    callisto_manifests::atomic::atomic_write(&target_file, content).unwrap();

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

    // Spec §G.6.7: Linked group members sync release severity (Minor), yielding 1.5.0 for pkg_a and 2.8.0 for pkg_b
    assert_eq!(target_a, "1.5.0");
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
        tags,
        runner: &runner,
    };

    let opts = TagOptions {
        dry_run: true,
        floating_major: false,
    };
    let report = callisto_graph::commands::create_tags_with_options(&ws, &plan, &opts).unwrap();
    assert_eq!(report.created_tags.len(), 1);
    assert_eq!(
        report.created_tags[0].tag_name.as_str(),
        "callisto-cli@0.2.0"
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
        tags,
        runner: &runner,
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

#[test]
fn test_snapshot_version_template_placeholders() {
    use callisto_graph::commands::plan_snapshot;

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
                stdout: "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0\n".to_string(),
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
        tags,
        runner: &runner,
    };

    let (_plan, report) = plan_snapshot(&ws, "canary").unwrap();
    assert_eq!(report.schema_version, callisto_model::SCHEMA_VERSION);
    assert!(report.snapshot_tag.contains("canary"));
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
        tags,
        runner: &runner,
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
