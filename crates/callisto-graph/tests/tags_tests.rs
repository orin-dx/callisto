mod fixtures;
use callisto_model::ApplyPermit;
use fixtures::GraphBuilder;
use std::cell::OnceCell;

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
    let git = callisto_vcs::GitAccess::discover(ws_dir.path(), &runner);
    let tags = callisto_graph::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: ws_dir.path().to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        git: OnceCell::from(git),
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
    let git = callisto_vcs::GitAccess::discover(ws_dir.path(), &runner);
    let tags = callisto_graph::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: ws_dir.path().to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        git: OnceCell::from(git),
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
    // (`Some(message)` branch: `tag -a -m <message> -- <name> <sha>`,
    // with a `--` in front of `<name>` so a leading-`-` ref name can never
    // be misread as a `git tag` flag)...
    let expected_create_call: Vec<String> = vec![
        "tag".into(),
        "-a".into(),
        "-m".into(),
        "Release callisto-cli@1.0.0".into(),
        "--".into(),
        "callisto-cli@1.0.0".into(),
        sha.as_str().to_string(),
    ];
    assert!(
        calls.contains(&expected_create_call),
        "expected an annotated `git tag -a` create call, got: {calls:?}"
    );

    // ...and to force-move the floating major tag, matching
    // `GitRepository::create_floating_major`'s unconditional-overwrite
    // (`PreviousValue::Any`) semantics, with the same `--` defense.
    let expected_floating_call: Vec<String> = vec![
        "tag".into(),
        "-f".into(),
        "--".into(),
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
    let git = callisto_vcs::GitAccess::discover(ws_dir.path(), &runner);
    let tags = callisto_graph::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
    let ws = callisto_graph::Workspace {
        root: ws_dir.path().to_path_buf(),
        config: cfg,
        graph,
        tags: OnceCell::from(tags),
        git: OnceCell::from(git),
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
