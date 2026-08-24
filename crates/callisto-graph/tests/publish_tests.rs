mod fixtures;
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_model::{DepKind, DepSpec, PackageId, PublishTarget};
use fixtures::{GraphBuilder, PackageBuilder};
use std::cell::OnceCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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

struct FailingRunner;
impl callisto_model::CommandRunner for FailingRunner {
    fn run(
        &self,
        program: &str,
        _args: &[&str],
        _cwd: &std::path::Path,
    ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
        Err(callisto_model::CommandError::NotFound {
            program: program.to_string(),
        })
    }
}

fn init_git_repo(dir: &std::path::Path) {
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git should run");
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test"]);
    run(&["config", "commit.gpgsign", "false"]);
    run(&["config", "tag.gpgsign", "false"]);
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "init"]);
}

/// An npm package whose `package.json` declares both `os` and `cpu` constraints
/// is a napi platform package and must appear in `npm_platform_packages`, not
/// in `npm_main_packages`. Before the walk.rs fix, `ManifestRole::Platform` was
/// never assigned so every npm package landed in `npm_main_packages`.
#[test]
fn napi_platform_package_is_classified_as_npm_platform_packages() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // A napi platform package: declares os and cpu constraints.
    std::fs::write(
        root.join("package.json"),
        r#"{"name":"@scope/my-lib-linux-x64-gnu","version":"1.0.0","os":["linux"],"cpu":["x64"]}"#,
    )
    .unwrap();

    // A real git repo so that tags() initialisation succeeds.
    init_git_repo(root);

    let runner = DummyRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner)
        .expect("workspace load should succeed for platform npm package");

    let plan =
        plan_publish(&ws, &PublishOptions::default()).expect("plan_publish should succeed for platform npm package");

    assert_eq!(
        plan.npm_main_packages.len(),
        0,
        "platform package must NOT be in npm_main_packages; got: {:?}",
        plan.npm_main_packages.iter().map(|p| &p.name).collect::<Vec<_>>()
    );
    assert_eq!(
        plan.npm_platform_packages.len(),
        1,
        "platform package must be in npm_platform_packages; got main_packages: {:?}",
        plan.npm_main_packages.iter().map(|p| &p.name).collect::<Vec<_>>()
    );
}

/// Plan publish must emit Cargo crates in dependency-first (topological) order:
/// a dependency must appear before every package that depends on it so that
/// downstream consumers can reference the already-published version.
///
/// Graph: pkg-c (no deps) <- pkg-b <- pkg-a
/// Expected rust_crates order: [pkg-c, pkg-b, pkg-a]
#[test]
fn test_publish_plan_uses_correct_topological_order() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let runner = DummyRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    // Write Cargo.toml manifests for all three packages so base_versions() can read them.
    for name in &["pkg-a", "pkg-b", "pkg-c"] {
        let pkg_dir = root.join(name);
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
    }

    // A real git repo is needed so that tags() initialisation does not fail.
    // No tags are created, so every package will be considered a release candidate
    // (tag_match = false -> is_release = true).
    init_git_repo(root);

    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();
    let pkg_c = PackageId::parse("pkg-c").unwrap();

    // Build graph: pkg-a -> pkg-b -> pkg-c (pkg-c has no dependencies).
    let graph = GraphBuilder::new()
        .package(pkg_a.clone(), |p: PackageBuilder| {
            p.publish_to(vec![PublishTarget::CratesIo])
        })
        .package(pkg_b.clone(), |p: PackageBuilder| {
            p.publish_to(vec![PublishTarget::CratesIo])
        })
        .package(pkg_c.clone(), |p: PackageBuilder| {
            p.publish_to(vec![PublishTarget::CratesIo])
        })
        .edge(
            pkg_a.clone(),
            pkg_b.clone(),
            DepKind::Runtime,
            DepSpec::Opaque("1.0.0".to_string()),
        )
        .edge(
            pkg_b.clone(),
            pkg_c.clone(),
            DepKind::Runtime,
            DepSpec::Opaque("1.0.0".to_string()),
        )
        .build()
        .unwrap();

    let cfg = callisto_graph::config::load(&root.join("callisto.toml")).unwrap();
    let git = callisto_vcs::GitAccess::discover(root, &runner);
    let tags = callisto_graph::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        git: OnceCell::from(git),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish should succeed");

    // All three packages should be in the plan (no tags => all are release candidates).
    assert_eq!(
        plan.rust_crates.len(),
        3,
        "expected all three packages in the plan; got: {:?}",
        plan.rust_crates.iter().map(|c| &c.name).collect::<Vec<_>>()
    );

    let names: Vec<&str> = plan.rust_crates.iter().map(|c| c.name.as_str()).collect();

    // pkg-c must appear before pkg-b (pkg-b depends on pkg-c).
    let pos_c = names
        .iter()
        .position(|&n| n == "pkg-c")
        .expect("pkg-c missing from plan");
    let pos_b = names
        .iter()
        .position(|&n| n == "pkg-b")
        .expect("pkg-b missing from plan");
    let pos_a = names
        .iter()
        .position(|&n| n == "pkg-a")
        .expect("pkg-a missing from plan");

    assert!(
        pos_c < pos_b,
        "pkg-c must precede pkg-b in publish plan (dependency first); order: {names:?}"
    );
    assert!(
        pos_b < pos_a,
        "pkg-b must precede pkg-a in publish plan (dependency first); order: {names:?}"
    );
}

// ---- PUB-001 regression guard -----------------------------------------------

/// A CommandRunner that captures the args slice of every call.
struct ArgsCapturingRunner {
    captured: Arc<Mutex<Vec<Vec<String>>>>,
}

impl callisto_model::CommandRunner for ArgsCapturingRunner {
    fn run(
        &self,
        _program: &str,
        args: &[&str],
        _cwd: &std::path::Path,
    ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
        self.captured
            .lock()
            .unwrap()
            .push(args.iter().map(|s| s.to_string()).collect());
        Ok(callisto_model::CommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

/// Regression guard for PUB-001: `SubprocessRegistryClient::publish()` silently
/// routes private-registry crates to crates.io when `load_plan()` has NOT been
/// called — because `cargo_registry` is empty and the lookup returns `None`.
///
/// The CLI's `handle()` must call `client.load_plan(&plan)` (line 127 of
/// `crates/callisto-cli/src/commands/publish.rs`) before passing the client to
/// the orchestrator. This test documents both sides of that invariant:
///
/// - WITH `load_plan`: `--registry cloudsmith` appears in the cargo args.
/// - WITHOUT `load_plan`: `--registry` is absent (crates.io fallback — wrong).
///
/// If the positive assertion ever fails, the CLI lost its `load_plan()` call or
/// `SubprocessRegistryClient` changed how it threads registry metadata.
#[test]
fn pub_001_load_plan_required_for_private_registry_routing() {
    use callisto_graph::commands::{
        AlwaysRetryPolicy, PublishOrchestrator, SubprocessRegistryClient, SystemTimeProvider,
    };
    use callisto_model::{
        ApplyPermit, CratePublish, PublishPlan, RegistryKey, Version, VersionGrammar, SCHEMA_VERSION,
    };

    let v = Version::parse("1.0.0", VersionGrammar::SemVer).unwrap();
    let plan = PublishPlan {
        schema_version: SCHEMA_VERSION,
        rust_crates: vec![CratePublish {
            name: "my-crate".to_string(),
            version: v,
            publish_to: RegistryKey("cloudsmith".to_string()),
            registry: Some("cloudsmith".to_string()),
            package_dir: None,
        }],
        npm_main_packages: vec![],
        npm_platform_packages: vec![],
        pypi_packages: vec![],
        releases: vec![],
        diagnostics: vec![],
    };
    let permit = ApplyPermit::force_for_tests();

    // --- WITH load_plan (correct path) ---
    let captured = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let mut client = SubprocessRegistryClient::new(
        ArgsCapturingRunner {
            captured: Arc::clone(&captured),
        },
        PathBuf::from("/workspace"),
    );
    client.load_plan(&plan);
    let orch = PublishOrchestrator::new(client, AlwaysRetryPolicy, SystemTimeProvider);
    drop(orch.execute(&plan, &permit));
    let recorded = captured.lock().unwrap().clone();
    let registry_present = recorded.iter().any(|args| args.contains(&"--registry".to_string()));
    assert!(
        registry_present,
        "with load_plan: --registry must appear in cargo args for private-registry crate; captured: {recorded:?}"
    );

    // --- WITHOUT load_plan (documents the regression this guard prevents) ---
    let captured2 = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let client2 = SubprocessRegistryClient::new(
        ArgsCapturingRunner {
            captured: Arc::clone(&captured2),
        },
        PathBuf::from("/workspace"),
    );
    let orch2 = PublishOrchestrator::new(client2, AlwaysRetryPolicy, SystemTimeProvider);
    drop(orch2.execute(&plan, &permit));
    let recorded2 = captured2.lock().unwrap().clone();
    let registry_absent = !recorded2.iter().any(|args| args.contains(&"--registry".to_string()));
    assert!(
        registry_absent,
        "without load_plan: --registry must be absent (crates.io fallback — this is the PUB-001 regression); captured: {recorded2:?}"
    );
}

// ---- F-04: npm registry from PublishTarget propagated to plan ----------------

/// A package whose `publishConfig.registry` in `package.json` matches a URL
/// the operator has explicitly approved via `[registries]` in `callisto.toml`
/// must produce a `NpmMainPublish` plan entry whose `registry` field carries
/// that URL. Before the SSRF fix, `plan_publish` always set `registry: None`;
/// after the SSRF fix, an operator-approved registry must still be propagated
/// (this is the legitimate private-registry case, distinct from an
/// unapproved override -- see `unapproved_publish_config_registry_is_rejected`
/// below).
#[test]
fn npm_registry_from_publish_target_is_propagated_to_plan() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // npm's `publishConfig.registry` in package.json is the standard mechanism
    // for targeting a private npm registry. The manifest editor must read it
    // and return PublishTarget::Npm { registry: Some(url) }, and plan_publish
    // must propagate that URL to the NpmMainPublish entry's `registry` field
    // -- but only because callisto.toml below explicitly approves this exact
    // URL via [registries]. Without that operator approval, this same
    // publishConfig.registry value must be rejected (SSRF guard).
    std::fs::write(
        root.join("package.json"),
        r#"{"name":"my-pkg","version":"1.0.0","publishConfig":{"registry":"https://npm.my-org.example.com"}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("callisto.toml"),
        "[registries.my-org-npm]\nkind = \"npm\"\nurl = \"https://npm.my-org.example.com\"\n",
    )
    .unwrap();
    init_git_repo(root);

    let runner = DummyRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load");
    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish");

    assert_eq!(plan.npm_main_packages.len(), 1, "expected one npm_main_packages entry");
    assert_eq!(
        plan.npm_main_packages[0].registry.as_deref(),
        Some("https://npm.my-org.example.com"),
        "publishConfig.registry from package.json must appear in plan entry when operator-approved; got {:?}",
        plan.npm_main_packages[0].registry
    );
}

// ---- SSRF guard: publishConfig.registry is manifest-controlled, not trusted ----

/// `publishConfig.registry` in `package.json` is data a PR author controls in
/// their own manifest -- it is NOT operator-controlled config. If a package
/// sets it to an arbitrary URL and there is no matching entry in the
/// operator's `[registries]` table in `callisto.toml`, `plan_publish` must
/// reject the plan rather than silently propagating the attacker-chosen URL
/// through to `npm publish --registry <url>` / `npm view --registry <url>`,
/// both of which run with `NPM_TOKEN` live in the environment in CI.
#[test]
fn unapproved_publish_config_registry_is_rejected() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // No callisto.toml at all -- the operator has configured no custom
    // registries, so this override must be rejected outright.
    std::fs::write(
        root.join("package.json"),
        r#"{"name":"my-pkg","version":"1.0.0","publishConfig":{"registry":"https://attacker.example/"}}"#,
    )
    .unwrap();
    init_git_repo(root);

    let runner = DummyRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load");

    let result = plan_publish(&ws, &PublishOptions::default());

    assert!(
        result.is_err(),
        "plan_publish must reject an unapproved publishConfig.registry override, got: {result:?}"
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("attacker.example"),
        "error must name the rejected URL; got: {msg}"
    );
}

/// A non-`https` `publishConfig.registry` scheme must always be rejected,
/// even if the operator happens to have approved that same host over
/// `https` in `[registries]` -- scheme downgrade is itself the attack this
/// guard defends against.
#[test]
fn non_https_publish_config_registry_is_rejected() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    std::fs::write(
        root.join("package.json"),
        r#"{"name":"my-pkg","version":"1.0.0","publishConfig":{"registry":"http://npm.my-org.example.com"}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("callisto.toml"),
        "[registries.my-org-npm]\nkind = \"npm\"\nurl = \"http://npm.my-org.example.com\"\n",
    )
    .unwrap();
    init_git_repo(root);

    let runner = DummyRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load");

    let result = plan_publish(&ws, &PublishOptions::default());

    assert!(
        result.is_err(),
        "plan_publish must reject a non-https publishConfig.registry scheme, got: {result:?}"
    );
}

// ---- A2/F-005: scoped npm packages must default to public access ---------------

/// npm's ecosystem default for `@scope/pkg` packages is `restricted`, which
/// requires a paid org plan on the public registry and returns a 402 on first
/// publish for free accounts. `plan_publish` must set `access: Some(Public)`
/// for any package whose name starts with `@`, and leave `access: None` for
/// unscoped packages (so npm uses its own default, which is public for those).
#[test]
fn scoped_npm_package_gets_public_access_in_plan() {
    use callisto_graph::commands::{plan_publish, PublishOptions};
    use callisto_model::NpmAccess;

    // -- Scoped package: @scope/my-lib --
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        std::fs::write(
            root.join("package.json"),
            r#"{"name":"@scope/my-lib","version":"1.0.0"}"#,
        )
        .unwrap();
        init_git_repo(root);

        let runner = DummyRunner;
        let locator = callisto_graph::locate::IgnoreWalkLocator::new(root);
        let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load");
        let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish");

        assert_eq!(plan.npm_main_packages.len(), 1);
        let pkg = &plan.npm_main_packages[0];
        assert_eq!(
            pkg.access,
            Some(NpmAccess::Public),
            "scoped package @scope/my-lib must get access: Public, got {:?}",
            pkg.access
        );
    }

    // -- Unscoped package: my-lib (no @ prefix) --
    {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        std::fs::write(root.join("package.json"), r#"{"name":"my-lib","version":"1.0.0"}"#).unwrap();
        init_git_repo(root);

        let runner = DummyRunner;
        let locator = callisto_graph::locate::IgnoreWalkLocator::new(root);
        let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load");
        let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish");

        assert_eq!(plan.npm_main_packages.len(), 1);
        let pkg = &plan.npm_main_packages[0];
        assert_eq!(
            pkg.access, None,
            "unscoped package my-lib must get access: None, got {:?}",
            pkg.access
        );
    }
}

// ---- Git discovery failure emits diagnostic ----------------------------------

/// When `plan_publish` cannot resolve a HEAD SHA (because the workspace is not
/// inside a git repository, or because the git repo has no commits), the
/// `releases` list in the resulting plan will be empty. Previously this was
/// silent — the operator had no indication why no release entries appeared.
///
/// The fix: emit a `GitDiscoveryFailed` diagnostic with `severity: warning` so
/// the operator can see what went wrong while still receiving a usable plan.
#[test]
fn non_git_workspace_emits_git_diagnostic() {
    use callisto_graph::commands::{plan_publish, PublishOptions};
    use callisto_model::DiagnosticCode;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // An npm package so the plan has something to work with — but deliberately
    // NO git init, so git discovery fails.
    std::fs::write(root.join("package.json"), r#"{"name":"my-pkg","version":"1.0.0"}"#).unwrap();

    let runner = DummyRunner;
    let locator = callisto_graph::locate::IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load");

    let plan =
        plan_publish(&ws, &PublishOptions::default()).expect("plan_publish must succeed even without a git repo");

    // No releases can be recorded without a HEAD SHA.
    assert!(
        plan.releases.is_empty(),
        "releases must be empty without a git repo; got: {:?}",
        plan.releases
    );

    let has_git_diagnostic = plan
        .diagnostics
        .iter()
        .any(|d| d.code == DiagnosticCode::GitDiscoveryFailed);

    assert!(
        has_git_diagnostic,
        "missing git repo must produce a GitDiscoveryFailed diagnostic; got: {:?}",
        plan.diagnostics
    );
}

// ---- F-06: changeset read errors must surface as diagnostics -----------------

/// When `.changeset/pre.json` or changeset files are malformed, `plan_publish`
/// previously called `.ok()` and silently swallowed the error. The plan would
/// then contain no version bumps with no indication why, making the output
/// misleading (publishing stale versions, no explanation).
///
/// The fix: capture the error and emit a `ChangesetReadError` diagnostic with
/// `severity: warning` so the user sees what went wrong while still getting
/// a usable (partial) publish plan.
#[test]
fn malformed_changeset_surfaces_as_diagnostic_not_silent_drop() {
    use callisto_graph::commands::{plan_publish, PublishOptions};
    use callisto_model::DiagnosticCode;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // A simple npm package so plan_publish has something to work with.
    std::fs::write(root.join("package.json"), r#"{"name":"my-pkg","version":"1.0.0"}"#).unwrap();

    // Write a malformed pre.json that will cause plan_version to fail.
    let cs_dir = root.join(".changeset");
    std::fs::create_dir_all(&cs_dir).unwrap();
    std::fs::write(cs_dir.join("pre.json"), "{ this is not valid json }").unwrap();

    init_git_repo(root);

    let runner = DummyRunner;
    let locator = callisto_graph::locate::IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load");

    let plan =
        plan_publish(&ws, &PublishOptions::default()).expect("plan_publish must succeed even with malformed changeset");

    let has_changeset_diagnostic = plan
        .diagnostics
        .iter()
        .any(|d| d.code == DiagnosticCode::ChangesetReadError);

    assert!(
        has_changeset_diagnostic,
        "malformed changeset must produce a ChangesetReadError diagnostic; got: {:?}",
        plan.diagnostics
    );
}

// ---- PUB-006: --package filter (only publish named packages) -----------------

/// When `PublishOptions::only` is populated, `plan_publish` must exclude all
/// packages not in the allowlist from every output list (`rust_crates`,
/// `npm_main_packages`, `npm_platform_packages`, `pypi_packages`).
///
/// Before this fix, `PublishOptions` was an empty struct — there was no way
/// to subset a publish run to a single package without editing the plan by hand.
#[test]
fn publish_options_only_filter_excludes_unlisted_packages() {
    use callisto_graph::commands::{plan_publish, PublishOptions};
    use callisto_model::{DepKind, DepSpec, PackageId, PublishTarget};

    let runner = DummyRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    // Two cargo packages.
    for name in &["pkg-a", "pkg-b"] {
        let pkg_dir = root.join(name);
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
    }

    init_git_repo(root);

    let pkg_a = PackageId::parse("pkg-a").unwrap();
    let pkg_b = PackageId::parse("pkg-b").unwrap();

    let graph = GraphBuilder::new()
        .package(pkg_a.clone(), |p: PackageBuilder| {
            p.publish_to(vec![PublishTarget::CratesIo])
        })
        .package(pkg_b.clone(), |p: PackageBuilder| {
            p.publish_to(vec![PublishTarget::CratesIo])
        })
        .edge(
            pkg_a.clone(),
            pkg_b.clone(),
            DepKind::Runtime,
            DepSpec::Opaque("1.0.0".to_string()),
        )
        .build()
        .unwrap();

    use std::cell::OnceCell;
    let cfg = callisto_graph::config::load(&root.join("callisto.toml")).unwrap();
    let git = callisto_vcs::GitAccess::discover(root, &runner);
    let tags = callisto_graph::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        git: OnceCell::from(git),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    // Only publish pkg-a; pkg-b must be excluded.
    let opts = PublishOptions {
        only: vec!["pkg-a".to_string()],
    };
    let plan = plan_publish(&ws, &opts).expect("plan_publish should succeed");

    assert_eq!(
        plan.rust_crates.len(),
        1,
        "only pkg-a should be in the plan; got: {:?}",
        plan.rust_crates.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
    assert_eq!(plan.rust_crates[0].name, "pkg-a", "the single plan entry must be pkg-a");
}

/// When `--package` is passed with a name that does not exist in the workspace,
/// the plan must return an error rather than silently producing an empty plan.
/// A silent empty plan exits 0, causing CI to report "nothing to publish"
/// instead of alerting the operator to the typo.
#[test]
fn publish_options_only_unknown_package_returns_error() {
    use callisto_graph::commands::{plan_publish, PublishOptions};
    use callisto_graph::error::GraphError;
    use callisto_model::{PackageId, PublishTarget};

    let runner = DummyRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    let pkg_dir = root.join("real-crate");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    std::fs::write(
        pkg_dir.join("Cargo.toml"),
        "[package]\nname = \"real-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    init_git_repo(root);

    let pkg_id = PackageId::parse("real-crate").unwrap();
    let graph = GraphBuilder::new()
        .package(pkg_id.clone(), |p: PackageBuilder| {
            p.publish_to(vec![PublishTarget::CratesIo])
        })
        .build()
        .unwrap();

    use std::cell::OnceCell;
    let cfg = callisto_graph::config::load(&root.join("callisto.toml")).unwrap();
    let git = callisto_vcs::GitAccess::discover(root, &runner);
    let tags = callisto_graph::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        git: OnceCell::from(git),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let opts = PublishOptions {
        only: vec!["typo-crate".to_string()],
    };
    let err = plan_publish(&ws, &opts).expect_err("plan_publish with an unknown --package name must return an error");
    assert!(
        matches!(err, GraphError::UnknownPackage { .. }),
        "unknown --package name must produce GraphError::UnknownPackage; got: {err:?}"
    );
}

// ---- PUB-011: ws.tags() failure must be soft (emit diagnostic, continue) -------

/// When the git binary is completely unavailable (runner returns
/// `CommandError::NotFound` for every shell invocation) and gix also cannot
/// discover a repository (no `.git` directory), `plan_publish` must NOT
/// hard-propagate the resulting `VcsError`. Instead it must return a
/// complete `PublishPlan` carrying a `GitDiscoveryFailed` diagnostic, and
/// treat every un-tagged package as a release candidate (`tag_match = false`).
///
/// Before this fix, `ws.tags()?` inside the topo loop would propagate the
/// `Err` as a hard `GraphError::Vcs`, crashing `plan_publish` immediately
/// after it had already emitted a soft `GitDiscoveryFailed` diagnostic for the
/// `head_sha` failure — a contradictory and misleading error sequence.
#[test]
fn plan_publish_succeeds_when_git_binary_unavailable() {
    use callisto_graph::commands::{plan_publish, PublishOptions};
    use callisto_model::DiagnosticCode;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // A Cargo package. Deliberately NO init_git_repo — gix will fail to
    // discover a repo. The FailingRunner also fails for shell git calls,
    // simulating an environment with no git binary installed.
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"my-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let runner = FailingRunner;
    let locator = callisto_graph::locate::IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner)
        .expect("workspace load must not require git");

    let plan = plan_publish(&ws, &PublishOptions::default())
        .expect("plan_publish must succeed even when git is completely unavailable");

    let has_git_diagnostic = plan
        .diagnostics
        .iter()
        .any(|d| d.code == DiagnosticCode::GitDiscoveryFailed);

    assert!(
        has_git_diagnostic,
        "git unavailability must produce a GitDiscoveryFailed diagnostic; got: {:?}",
        plan.diagnostics
    );
}

/// Consolidation regression test: `plan_publish`'s `head_sha` resolution and
/// `ws.tags()`'s tag-list resolution now go through the single shared
/// `Workspace::git_access()` instead of two independent `GitAccess::discover`
/// calls. Proves that instance's `CommandRunner` shell fallback (exercised
/// when gix cannot discover a repository, as is permanently the case on
/// `wasm32`) is enough on its own to produce a real, populated
/// `ReleaseEntry` end-to-end -- not just that the plan degrades gracefully
/// when git is totally unavailable (see
/// `plan_publish_succeeds_when_git_binary_unavailable`, which uses a
/// `FailingRunner`). Mirrors `snapshot_tests.rs`'s analogous
/// `test_snapshot_resolves_head_sha_via_command_runner_fallback_when_gix_unavailable`.
#[test]
fn plan_publish_populates_release_entry_via_command_runner_fallback_when_gix_unavailable() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    struct FakeHeadShaRunner(String);
    impl callisto_model::CommandRunner for FakeHeadShaRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            assert_eq!(program, "git");
            match args {
                ["rev-parse", "HEAD"] => Ok(callisto_model::CommandOutput {
                    exit_code: Some(0),
                    stdout: format!("{}\n", self.0),
                    stderr: String::new(),
                }),
                ["tag", "--list"] => Ok(callisto_model::CommandOutput {
                    exit_code: Some(0),
                    stdout: String::new(),
                    stderr: String::new(),
                }),
                other => panic!("unexpected git invocation: {other:?}"),
            }
        }
    }

    let head_sha = "c".repeat(40);
    let runner = FakeHeadShaRunner(head_sha.clone());
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    // Deliberately no `git init`: gix discovery must fail here, forcing the
    // shell fallback for BOTH head_sha and tag-list resolution -- through
    // the same shared `GitAccess`.
    assert!(
        callisto_vcs::GitRepository::discover(root).is_err(),
        "test fixture must not be discoverable as a Git repo"
    );

    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"my-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let locator = callisto_graph::locate::IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner)
        .expect("workspace load must not require git");

    let plan = plan_publish(&ws, &PublishOptions::default())
        .expect("plan_publish must succeed via the CommandRunner fallback when gix cannot discover a repo");

    assert!(
        !plan
            .diagnostics
            .iter()
            .any(|d| d.code == callisto_model::DiagnosticCode::GitDiscoveryFailed),
        "the CommandRunner fallback must resolve git successfully, so no GitDiscoveryFailed \
         diagnostic should be emitted: got {:?}",
        plan.diagnostics
    );

    let pkg_id = PackageId::parse("my-crate").unwrap();
    let release = plan.releases.iter().find(|r| r.package == pkg_id).unwrap_or_else(|| {
        panic!(
            "expected a ReleaseEntry for my-crate via the CommandRunner fallback, got: {:?}",
            plan.releases
        )
    });

    assert_eq!(
        release.sha.as_str(),
        head_sha,
        "the release entry's sha must be the one resolved via the CommandRunner fallback"
    );
}

// ---- PUB-012: release-entry block must use pre-computed tag_index not ws.tags()? -----

/// When `ws.tags()` fails consistently (e.g. `TagIndex::build` hits a
/// `ModelError::mixed_version_grammars` for a package whose canonical
/// manifests span two ecosystems), `plan_publish`'s soft handler correctly
/// sets `tag_index = None` and emits a diagnostic. But there is a second,
/// unguarded `ws.tags()?` inside the release-entry building block (the
/// `if let Some(ref sha) = head_sha` branch). When `head_sha` succeeds (a
/// real `.git` directory is present), this inner call re-triggers the same
/// error and propagates it as a hard `GraphError`, crashing `plan_publish`
/// despite the earlier graceful-degradation path.
///
/// After the fix, the release-entry block consults `tag_index` instead of
/// calling `ws.tags()` again — so when `tag_index` is `None`, no release
/// entry is pushed and the plan succeeds.
#[test]
fn plan_publish_release_entry_block_does_not_hard_fail_when_tag_index_is_none() {
    use callisto_graph::commands::{plan_publish, PublishOptions};
    use callisto_model::{DiagnosticCode, ManifestDecl, ManifestFormat, ManifestRole};

    let runner = DummyRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    // Write only Cargo.toml on disk — base_versions() reads the first canonical
    // manifest (CargoToml) and succeeds. PyprojectToml just exists as a ManifestDecl
    // metadata entry; base_versions() never tries to open it.
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"my-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    // Real git repo — head_sha() succeeds via gix, so the release-entry block
    // is entered (head_sha = Some(sha)). Without the fix, ws.tags()? in that
    // block propagates the mixed-grammar error as a hard GraphError.
    init_git_repo(root);

    // Package with BOTH CargoToml AND PyprojectToml as Canonical manifests.
    // base_versions() reads Cargo.toml (first) → succeeds.
    // version_grammar() sees [SemVer, PEP440] → Err(mixed_version_grammars).
    // TagIndex::build hits this error → ws.tags() always returns Err.
    let pkg_id = PackageId::parse("my-crate").unwrap();
    let cargo_decl = ManifestDecl::new(
        PathBuf::from("Cargo.toml"),
        ManifestRole::Canonical,
        ManifestFormat::CargoToml,
    )
    .unwrap();
    let pyproject_decl = ManifestDecl::new(
        PathBuf::from("pyproject.toml"),
        ManifestRole::Canonical,
        ManifestFormat::PyprojectToml,
    )
    .unwrap();

    let graph = GraphBuilder::new()
        .package(pkg_id.clone(), |p: PackageBuilder| {
            p.manifests(vec![cargo_decl, pyproject_decl])
                .publish_to(vec![PublishTarget::CratesIo])
        })
        .build()
        .unwrap();

    let cfg = callisto_graph::config::load(&root.join("callisto.toml")).unwrap();
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::new(),
        git: OnceCell::new(),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let plan = plan_publish(&ws, &PublishOptions::default()).expect(
        "plan_publish must succeed even when ws.tags() consistently fails \
         (mixed version grammar) while head_sha is available",
    );

    // Package is still included as a release candidate (is_release = true,
    // tag_match = false since tag_index = None).
    assert_eq!(
        plan.rust_crates.len(),
        1,
        "package must appear in rust_crates even when tag_index unavailable; got: {:?}",
        plan.rust_crates.iter().map(|c| &c.name).collect::<Vec<_>>()
    );

    // Release entries must be empty — without a working tag_index we cannot
    // generate tag names, so they are omitted (consistent with the diagnostic).
    assert!(
        plan.releases.is_empty(),
        "releases must be empty when tag_index is unavailable; got: {:?}",
        plan.releases
    );

    let has_git_diagnostic = plan
        .diagnostics
        .iter()
        .any(|d| d.code == DiagnosticCode::GitDiscoveryFailed);
    assert!(
        has_git_diagnostic,
        "must emit GitDiscoveryFailed diagnostic when ws.tags() fails; got: {:?}",
        plan.diagnostics
    );
}

// ---- PUB-014: [[package]] publish-to override must be applied to the package in the graph ----

/// When `callisto.toml` contains `[[package]] publish-to = ["none"]` for a
/// Cargo package, `plan_publish` must exclude that package from rust_crates.
///
/// Before this fix, `resolve.rs` always set `publish_to: None` for all
/// [[package]] blocks (raw_pkg.publish_to was never parsed), and `walk.rs`
/// never consulted `pkg_override.publish_to` even if it had been set.
/// The package would appear in rust_crates with its manifest-derived
/// `CratesIo` target, ignoring the operator's explicit override.
#[test]
fn package_override_publish_to_none_excludes_package_from_plan() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"my-private-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    // Override: this crate is internal and must not be published.
    std::fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"my-private-crate\"\npublish-to = [\"none\"]\n",
    )
    .unwrap();

    init_git_repo(root);

    let runner = DummyRunner;
    let locator = callisto_graph::locate::IgnoreWalkLocator::new(root);
    let ws =
        callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load should succeed");

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish should succeed");

    assert_eq!(
        plan.rust_crates.len(),
        0,
        "a package with [[package]] publish-to = [\"none\"] must NOT appear in rust_crates; \
         got: {:?}",
        plan.rust_crates.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
}

// ---- PUB-013: publishConfig.access:restricted must not be overridden by --access public ----

/// npm's `--access` CLI flag takes full precedence over `publishConfig.access`
/// in `package.json`. When a scoped package sets `publishConfig.access: "restricted"`
/// (marking it as a private package), callisto must NOT pass `--access public`
/// to the package manager — that would override the operator's intent and expose
/// the package publicly.
///
/// Before the fix, `plan_publish` unconditionally set `access = Some(NpmAccess::Public)`
/// for any package whose name starts with `@`, ignoring `publishConfig.access`.
/// The code comment claimed `publishConfig.access` would override `--access`, which
/// is false — `--access` always wins.
///
/// After the fix, `plan_publish` reads `publishConfig.access` and sets
/// `access = Some(NpmAccess::Restricted)` when it is "restricted", so that
/// the publish command correctly passes `--access restricted` (or the publish
/// client omits the flag and lets the package manager read `publishConfig`).
#[test]
fn scoped_npm_package_with_restricted_access_produces_restricted_access_in_plan() {
    use callisto_graph::commands::{plan_publish, PublishOptions};
    use callisto_model::NpmAccess;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // Scoped private package: publishConfig.access is explicitly "restricted".
    // Before the fix, plan_publish ignores this and sets access = Public.
    // After the fix, it must set access = Restricted.
    std::fs::write(
        root.join("package.json"),
        r#"{"name":"@corp/internal-sdk","version":"1.0.0","publishConfig":{"access":"restricted"}}"#,
    )
    .unwrap();
    init_git_repo(root);

    let runner = DummyRunner;
    let locator = callisto_graph::locate::IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load");

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish");

    assert_eq!(plan.npm_main_packages.len(), 1, "expected one npm_main_packages entry");
    assert_eq!(
        plan.npm_main_packages[0].access,
        Some(NpmAccess::Restricted),
        "publishConfig.access:restricted must produce NpmAccess::Restricted in plan; \
         got: {:?} — passing --access public would expose the private package",
        plan.npm_main_packages[0].access
    );
}

/// `PublishTarget::Npm.restricted: bool` collapsed "publishConfig.access
/// absent" and "publishConfig.access explicitly 'public'" into the same
/// `false` value, so an *unscoped* package's explicit `"public"` setting
/// was silently dropped (the only fallback that produced
/// `Some(NpmAccess::Public)` was the `@scope/name`-starts-with-`@` heuristic,
/// which never fires for an unscoped name). An operator who explicitly
/// wrote `publishConfig.access: "public"` on an unscoped package expects
/// that intent to be honoured, not silently discarded.
#[test]
fn unscoped_npm_package_with_explicit_public_access_produces_public_access_in_plan() {
    use callisto_graph::commands::{plan_publish, PublishOptions};
    use callisto_model::NpmAccess;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    std::fs::write(
        root.join("package.json"),
        r#"{"name":"my-unscoped-pkg","version":"1.0.0","publishConfig":{"access":"public"}}"#,
    )
    .unwrap();
    init_git_repo(root);

    let runner = DummyRunner;
    let locator = callisto_graph::locate::IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load");

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish");

    assert_eq!(plan.npm_main_packages.len(), 1);
    assert_eq!(
        plan.npm_main_packages[0].access,
        Some(NpmAccess::Public),
        "an unscoped package's explicit publishConfig.access:\"public\" must be \
         honoured, not silently dropped; got: {:?}",
        plan.npm_main_packages[0].access
    );
}

// ---- [[package-set]]: bulk config-override via glob pattern ----

/// When `callisto.toml` contains `[[package-set]] match = "pkg-*" publish-to = ["none"]`,
/// all packages whose name matches the glob must have their publish-to overridden.
///
/// This test uses two Cargo packages: "pkg-a" and "pkg-b". Both match "pkg-*".
/// The `[[package-set]]` rule must suppress both from rust_crates.
///
/// Before the fix, `walk.rs` never looked for a `[[package-set]]` fallback, so
/// all packages would appear in rust_crates regardless of any [[package-set]] rule.
#[test]
fn package_set_override_applies_to_all_matching_packages() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // Workspace with two members.
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"pkg-a\", \"pkg-b\"]\nresolver = \"2\"\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join("pkg-a")).unwrap();
    std::fs::write(
        root.join("pkg-a/Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join("pkg-b")).unwrap();
    std::fs::write(
        root.join("pkg-b/Cargo.toml"),
        "[package]\nname = \"pkg-b\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    // [[package-set]] suppresses both packages via glob.
    std::fs::write(
        root.join("callisto.toml"),
        "[[package-set]]\nmatch = \"pkg-*\"\npublish-to = [\"none\"]\n",
    )
    .unwrap();

    init_git_repo(root);

    let runner = DummyRunner;
    let locator = callisto_graph::locate::IgnoreWalkLocator::new(root);
    let ws =
        callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load should succeed");

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish should succeed");

    assert_eq!(
        plan.rust_crates.len(),
        0,
        "[[package-set]] match = \"pkg-*\" publish-to = [\"none\"] must suppress both pkg-a \
         and pkg-b; got: {:?}",
        plan.rust_crates.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
}

/// When both a `[[package]]` and a `[[package-set]]` rule match a package,
/// the `[[package]]` rule must take priority.
///
/// Setup: two packages "pkg-a" (explicit [[package]] publish-to = ["crates-io"])
/// and "pkg-b". A [[package-set]] match = "pkg-*" publish-to = ["none"] would
/// suppress both. But [[package]] for pkg-a must win, keeping it in the plan.
#[test]
fn package_rule_takes_priority_over_package_set_rule() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"pkg-a\", \"pkg-b\"]\nresolver = \"2\"\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join("pkg-a")).unwrap();
    std::fs::write(
        root.join("pkg-a/Cargo.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join("pkg-b")).unwrap();
    std::fs::write(
        root.join("pkg-b/Cargo.toml"),
        "[package]\nname = \"pkg-b\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    // pkg-a has an explicit [[package]] rule (publish); [[package-set]] suppresses all "pkg-*".
    std::fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"pkg-a\"\npublish-to = [\"crates-io\"]\n\n\
         [[package-set]]\nmatch = \"pkg-*\"\npublish-to = [\"none\"]\n",
    )
    .unwrap();

    init_git_repo(root);

    let runner = DummyRunner;
    let locator = callisto_graph::locate::IgnoreWalkLocator::new(root);
    let ws =
        callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load should succeed");

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish should succeed");

    let crate_names: Vec<&str> = plan.rust_crates.iter().map(|c| c.name.as_str()).collect();

    assert!(
        crate_names.contains(&"pkg-a"),
        "[[package]] publish-to = [\"crates-io\"] must override [[package-set]] \
         publish-to = [\"none\"] for pkg-a; pkg-a must appear in rust_crates"
    );
    assert!(
        !crate_names.contains(&"pkg-b"),
        "pkg-b has no [[package]] rule, so [[package-set]] publish-to = [\"none\"] \
         must suppress it; pkg-b must NOT appear in rust_crates"
    );
}

// ---- Fix: release-tag/ReleaseEntry gate decoupled from actual publish dispatch ----

/// `PublishTarget::GitHubRelease` has an `ecosystem()` of `None`, so it
/// passes the walk.rs ecosystem-mismatch check regardless of the package's
/// real ecosystem — but `plan_publish`'s dispatch loop has no real
/// implementation for it (only `CratesIo`/`Npm`/`Pypi` are dispatched).
///
/// Before this fix, the release-tag/`ReleaseEntry` gate only checked that
/// `publish_to` was non-empty and not all `PublishTarget::None`, completely
/// decoupled from whether anything was actually dispatchable. A package
/// configured with only `publish-to = ["github-release"]` would get a
/// `ReleaseEntry` (claiming a release happened) while zero registries were
/// ever contacted and no diagnostic was emitted.
///
/// After the fix: no `ReleaseEntry` is created for a package whose only
/// configured targets have no real dispatch, and an explicit
/// `PublishTargetNotImplemented` diagnostic is emitted instead.
#[test]
fn package_with_only_undispatchable_target_gets_no_release_entry_and_a_diagnostic() {
    use callisto_graph::commands::{plan_publish, PublishOptions};
    use callisto_model::{DiagnosticCode, ManifestDecl, ManifestFormat, ManifestRole};

    let runner = DummyRunner;
    let ws_dir = tempfile::tempdir().unwrap();
    let root = ws_dir.path();

    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"my-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    // Real git repo so head_sha() and tag_index both succeed — the
    // release-entry block is fully reachable, isolating this test to the
    // gate logic itself rather than the git-unavailable soft-fail path.
    init_git_repo(root);

    let pkg_id = PackageId::parse("my-crate").unwrap();
    let cargo_decl = ManifestDecl::new(
        PathBuf::from("Cargo.toml"),
        ManifestRole::Canonical,
        ManifestFormat::CargoToml,
    )
    .unwrap();

    let graph = GraphBuilder::new()
        .package(pkg_id.clone(), |p: PackageBuilder| {
            p.manifests(vec![cargo_decl])
                .publish_to(vec![PublishTarget::GitHubRelease])
        })
        .build()
        .unwrap();

    let cfg = callisto_graph::config::load(root).unwrap();
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::new(),
        git: OnceCell::new(),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let plan = plan_publish(&ws, &PublishOptions::default())
        .expect("plan_publish must succeed for a GitHubRelease-only package");

    assert!(
        plan.releases.iter().all(|r| r.package != pkg_id),
        "no ReleaseEntry must be created for a package whose only configured \
         target (GitHubRelease) has no real dispatch implementation; got: {:?}",
        plan.releases
    );

    let has_not_implemented_diagnostic = plan
        .diagnostics
        .iter()
        .any(|d| d.code == DiagnosticCode::PublishTargetNotImplemented && d.package.as_ref() == Some(&pkg_id));
    assert!(
        has_not_implemented_diagnostic,
        "expected a PublishTargetNotImplemented diagnostic for my-crate; got: {:?}",
        plan.diagnostics
    );
}

// ---- changelog_section population (AC-10, AC-11) ----

fn write_release_candidate_pkg(root: &std::path::Path, version: &str) {
    std::fs::write(
        root.join("Cargo.toml"),
        format!("[package]\nname = \"pkg\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
    )
    .unwrap();
}

#[test]
fn plan_publish_populates_changelog_section_when_heading_has_content() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_release_candidate_pkg(root, "1.2.3");
    std::fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## 1.2.3\n\nSome release notes here.\n\n## 1.2.2\n\nOlder notes.\n",
    )
    .unwrap();
    init_git_repo(root);

    let runner = DummyRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish must succeed");
    let pkg_id = PackageId::parse("pkg").unwrap();
    let release = plan
        .releases
        .iter()
        .find(|r| r.package == pkg_id)
        .unwrap_or_else(|| panic!("expected a ReleaseEntry for pkg, got: {:?}", plan.releases));

    assert_eq!(
        release.changelog_section.as_deref(),
        Some("Some release notes here."),
        "changelog_section must equal extract_section's trimmed output for the matching heading"
    );
}

#[test]
fn plan_publish_leaves_changelog_section_none_when_file_does_not_exist() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_release_candidate_pkg(root, "1.2.3");
    // Deliberately no CHANGELOG.md written.
    init_git_repo(root);

    let runner = DummyRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish must succeed");
    let pkg_id = PackageId::parse("pkg").unwrap();
    let release = plan
        .releases
        .iter()
        .find(|r| r.package == pkg_id)
        .unwrap_or_else(|| panic!("expected a ReleaseEntry for pkg, got: {:?}", plan.releases));

    assert_eq!(release.changelog_section, None, "AC-11: no changelog file on disk");
    assert!(
        plan.diagnostics
            .iter()
            .any(|d| d.code == callisto_model::DiagnosticCode::ChangelogSectionNotFound
                && d.package.as_ref() == Some(&pkg_id)),
        "expected a ChangelogSectionNotFound diagnostic naming pkg; got {:?}",
        plan.diagnostics
    );
}

// ---- regression: AC-10b (empty section), AC-12 (no matching heading) ----

#[test]
fn plan_publish_leaves_changelog_section_none_when_matched_heading_section_is_empty() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_release_candidate_pkg(root, "1.2.3");
    std::fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## 1.2.3\n\n\n\n## 1.2.2\n\nOlder notes.\n",
    )
    .unwrap();
    init_git_repo(root);

    let runner = DummyRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish must succeed");
    let pkg_id = PackageId::parse("pkg").unwrap();
    let release = plan.releases.iter().find(|r| r.package == pkg_id).unwrap();

    assert_eq!(
        release.changelog_section, None,
        "AC-10b: matched heading has empty content"
    );
    assert!(
        plan.diagnostics
            .iter()
            .any(|d| d.code == callisto_model::DiagnosticCode::ChangelogSectionNotFound
                && d.package.as_ref() == Some(&pkg_id)),
        "expected a ChangelogSectionNotFound diagnostic naming pkg; got {:?}",
        plan.diagnostics
    );
}

#[test]
fn plan_publish_leaves_changelog_section_none_when_no_matching_heading() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_release_candidate_pkg(root, "1.2.3");
    std::fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\n## 1.2.2\n\nOlder notes only, no 1.2.3 section.\n",
    )
    .unwrap();
    init_git_repo(root);

    let runner = DummyRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish must succeed");
    let pkg_id = PackageId::parse("pkg").unwrap();
    let release = plan.releases.iter().find(|r| r.package == pkg_id).unwrap();

    assert_eq!(
        release.changelog_section, None,
        "AC-12: no `## 1.2.3` heading in the file"
    );
    assert!(
        plan.diagnostics
            .iter()
            .any(|d| d.code == callisto_model::DiagnosticCode::ChangelogSectionNotFound
                && d.package.as_ref() == Some(&pkg_id)),
        "expected a ChangelogSectionNotFound diagnostic naming pkg; got {:?}",
        plan.diagnostics
    );
}

// ---- regression: AC-12c (unreadable / invalid UTF-8 changelog file) ----

#[test]
fn plan_publish_leaves_changelog_section_none_and_emits_read_error_for_invalid_utf8() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_release_candidate_pkg(root, "1.2.3");
    std::fs::write(root.join("CHANGELOG.md"), [0x23, 0x20, 0xFF, 0xFE, 0x0A]).unwrap();
    init_git_repo(root);

    let runner = DummyRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish must succeed");
    let pkg_id = PackageId::parse("pkg").unwrap();
    let release = plan.releases.iter().find(|r| r.package == pkg_id).unwrap();

    assert_eq!(
        release.changelog_section, None,
        "AC-12c: file exists but is not valid UTF-8"
    );
    assert!(
        plan.diagnostics.iter().any(
            |d| d.code == callisto_model::DiagnosticCode::ChangelogReadError && d.package.as_ref() == Some(&pkg_id)
        ),
        "expected a ChangelogReadError diagnostic naming pkg; got {:?}",
        plan.diagnostics
    );
    assert!(
        !plan
            .diagnostics
            .iter()
            .any(|d| d.code == callisto_model::DiagnosticCode::ChangelogSectionNotFound
                && d.package.as_ref() == Some(&pkg_id)),
        "an unreadable file must not also be reported as ChangelogSectionNotFound"
    );
}

#[test]
fn plan_publish_release_entry_is_prerelease_matches_version() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_release_candidate_pkg(root, "1.0.0-1");
    init_git_repo(root);

    let runner = DummyRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish must succeed");
    let pkg_id = PackageId::parse("pkg").unwrap();
    let release = plan
        .releases
        .iter()
        .find(|r| r.package == pkg_id)
        .unwrap_or_else(|| panic!("expected a ReleaseEntry for pkg, got: {:?}", plan.releases));

    assert!(
        release.is_prerelease,
        "SemVer 1.0.0-1 must be reported as a pre-release via Version::is_prerelease()"
    );
}

#[test]
fn plan_publish_semver_non_prerelease_has_is_prerelease_false_in_json() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_release_candidate_pkg(root, "1.2.3");
    init_git_repo(root);

    let runner = DummyRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish must succeed");
    let pkg_id = PackageId::parse("pkg").unwrap();
    let release = plan
        .releases
        .iter()
        .find(|r| r.package == pkg_id)
        .unwrap_or_else(|| panic!("expected a ReleaseEntry for pkg, got: {:?}", plan.releases));
    assert!(!release.is_prerelease, "SemVer 1.2.3 must not be a prerelease");

    let json = serde_json::to_value(&plan).unwrap();
    let entries = json["releases"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "expected exactly one release entry");
    assert_eq!(
        entries[0]["isPrerelease"],
        serde_json::json!(false),
        "isPrerelease key must be present in serialized JSON and equal false"
    );
}

#[test]
fn plan_publish_semver_regression_fixture_has_is_prerelease_true() {
    use callisto_graph::commands::{plan_publish, PublishOptions};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_release_candidate_pkg(root, "1.0.0-1");
    init_git_repo(root);

    let runner = DummyRunner;
    let locator = IgnoreWalkLocator::new(root);
    let ws = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner).unwrap();

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish must succeed");
    let pkg_id = PackageId::parse("pkg").unwrap();
    let release = plan
        .releases
        .iter()
        .find(|r| r.package == pkg_id)
        .unwrap_or_else(|| panic!("expected a ReleaseEntry for pkg, got: {:?}", plan.releases));
    assert!(
        release.is_prerelease,
        "SemVer 1.0.0-1 (numeric pre-release identifier, no alpha/beta/rc/pre/next substring) must be a prerelease"
    );

    let json = serde_json::to_value(&plan).unwrap();
    let entries = json["releases"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["isPrerelease"], serde_json::json!(true));
}

#[test]
fn plan_publish_pep440_dev_release_has_is_prerelease_true() {
    use callisto_graph::commands::{plan_publish, PublishOptions};
    use callisto_model::{ManifestDecl, ManifestFormat, ManifestRole, PublishTarget};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(
        root.join("pyproject.toml"),
        "[project]\nname = \"pkg\"\nversion = \"1.0.0.dev1\"\n",
    )
    .unwrap();
    init_git_repo(root);

    let runner = DummyRunner;
    let pkg_id = PackageId::parse("pkg").unwrap();
    let pyproject_decl = ManifestDecl::new(
        PathBuf::from("pyproject.toml"),
        ManifestRole::Canonical,
        ManifestFormat::PyprojectToml,
    )
    .unwrap();

    let graph = GraphBuilder::new()
        .package(pkg_id.clone(), |p: PackageBuilder| {
            p.manifests(vec![pyproject_decl])
                .publish_to(vec![PublishTarget::Pypi { index: None }])
        })
        .build()
        .unwrap();

    let cfg = callisto_graph::config::load(&root.join("callisto.toml")).unwrap();
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::new(),
        git: OnceCell::new(),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish must succeed");
    let release = plan
        .releases
        .iter()
        .find(|r| r.package == pkg_id)
        .unwrap_or_else(|| panic!("expected a ReleaseEntry for pkg, got: {:?}", plan.releases));
    assert!(
        release.is_prerelease,
        "PEP 440 1.0.0.dev1 (dev-release) must be a prerelease per pep440_dev_release_is_prerelease"
    );

    let json = serde_json::to_value(&plan).unwrap();
    let entries = json["releases"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["isPrerelease"], serde_json::json!(true));
}

#[test]
fn plan_publish_pep440_regression_fixture_has_is_prerelease_true() {
    use callisto_graph::commands::{plan_publish, PublishOptions};
    use callisto_model::{ManifestDecl, ManifestFormat, ManifestRole, PublishTarget};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(
        root.join("pyproject.toml"),
        "[project]\nname = \"pkg\"\nversion = \"1.2.3a1\"\n",
    )
    .unwrap();
    init_git_repo(root);

    let runner = DummyRunner;
    let pkg_id = PackageId::parse("pkg").unwrap();
    let pyproject_decl = ManifestDecl::new(
        PathBuf::from("pyproject.toml"),
        ManifestRole::Canonical,
        ManifestFormat::PyprojectToml,
    )
    .unwrap();

    let graph = GraphBuilder::new()
        .package(pkg_id.clone(), |p: PackageBuilder| {
            p.manifests(vec![pyproject_decl])
                .publish_to(vec![PublishTarget::Pypi { index: None }])
        })
        .build()
        .unwrap();

    let cfg = callisto_graph::config::load(&root.join("callisto.toml")).unwrap();
    let ws = callisto_graph::Workspace {
        root: root.to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::new(),
        git: OnceCell::new(),
        runner: &runner,
        manifest_cache: Default::default(),
        identity: callisto_graph::IdentityIndex::default(),
    };

    let plan = plan_publish(&ws, &PublishOptions::default()).expect("plan_publish must succeed");
    let release = plan
        .releases
        .iter()
        .find(|r| r.package == pkg_id)
        .unwrap_or_else(|| panic!("expected a ReleaseEntry for pkg, got: {:?}", plan.releases));
    assert!(
        release.is_prerelease,
        "PEP 440 1.2.3a1 (no dash, no alpha/beta/rc/pre/next substring) must be a prerelease -- the original bug"
    );

    let json = serde_json::to_value(&plan).unwrap();
    let entries = json["releases"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["isPrerelease"], serde_json::json!(true));
}
