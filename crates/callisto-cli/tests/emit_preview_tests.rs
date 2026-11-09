mod common;

use std::process::Command;

use callisto_cli::cli::{AddArgs, ComposePrBodyArgs, GlobalArgs, InitArgs, InitVersioning, OutputFormat, VersionArgs};
use callisto_cli::commands;
use callisto_graph::commands::PrBodyOptions;
use callisto_graph::infer::NoInference;

use common::setup_polyglot_git_repo;

fn version_args(emit_preview: Option<std::path::PathBuf>) -> VersionArgs {
    VersionArgs {
        refresh_lockfiles: false,
        no_refresh_lockfiles: true,
        strict: false,
        allow_empty_changesets: false,
        emit_decision: None,
        emit_preview,
    }
}

/// The body composed from the preview `version` wrote, after the changesets are consumed, is the body composed from
/// the pending changesets before it.
#[test]
fn a_body_composed_from_the_emitted_preview_matches_the_body_composed_before_versioning() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();
    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };
    assert!(Command::new("git")
        .args(["remote", "add", "origin", "https://github.com/example/core-crate.git"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    commands::init::handle(
        InitArgs {
            yes: true,
            versioning: Some(InitVersioning::Independent),
            ..Default::default()
        },
        &global,
    )
    .unwrap();
    commands::add::handle(
        AddArgs {
            packages: vec!["cargo/core-crate:minor".to_string()],
            summary: Some("Retry uploads.\n\nWith backoff.".to_string()),
        },
        &global,
    )
    .unwrap();

    let before = {
        let runner = callisto_cli::runner::CliCommandRunner;
        let ws = callisto_cli::workspace::load_workspace(&global, &runner).unwrap();
        callisto_graph::commands::compose_pr_body(&ws, &NoInference, &PrBodyOptions::default())
            .unwrap()
            .body
    };

    let out = tempfile::TempDir::new().unwrap();
    let preview_path = out.path().join("preview.json");
    commands::version::handle(version_args(Some(preview_path.clone())), &global).unwrap();
    let text = std::fs::read_to_string(&preview_path).expect("version wrote the preview");
    let preview = callisto_graph::release_preview::ReleasePreview::from_json(&text).unwrap();
    assert_eq!(preview.packages[0].name, "core-crate");
    assert_eq!(preview.packages[0].ecosystem, "cargo");

    let after = callisto_graph::commands::render_pr_body_from_preview(&preview, &PrBodyOptions::default()).body;
    assert_eq!(after, before);

    let status = commands::compose_pr_body::handle(
        ComposePrBodyArgs {
            existing_body: None,
            branch: None,
            preview: Some(preview_path.clone()),
            publish_plan: false,
        },
        &global,
    );
    assert_eq!(status.unwrap(), std::process::ExitCode::SUCCESS);
}

fn init_repo(global: &GlobalArgs) {
    assert!(Command::new("git")
        .args(["remote", "add", "origin", "https://github.com/example/core-crate.git"])
        .current_dir(&global.cwd)
        .status()
        .unwrap()
        .success());
    commands::init::handle(
        InitArgs {
            yes: true,
            versioning: Some(InitVersioning::Independent),
            ..Default::default()
        },
        global,
    )
    .unwrap();
}

/// `version` and `compose-pr-body` must agree on what a relative preview path means: it is relative to `--cwd`.
#[test]
fn a_relative_preview_path_means_cwd_for_both_commands() {
    let dir = setup_polyglot_git_repo();
    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: dir.path().to_path_buf(),
        dry_run: false,
    };
    init_repo(&global);
    commands::add::handle(
        AddArgs {
            packages: vec!["cargo/core-crate:minor".to_string()],
            summary: Some("Retry uploads.".to_string()),
        },
        &global,
    )
    .unwrap();

    commands::version::handle(version_args(Some("preview.json".into())), &global).unwrap();
    assert!(dir.path().join("preview.json").exists());

    let status = commands::compose_pr_body::handle(
        ComposePrBodyArgs {
            existing_body: None,
            branch: None,
            preview: Some("preview.json".into()),
            publish_plan: false,
        },
        &global,
    );
    assert_eq!(status.unwrap(), std::process::ExitCode::SUCCESS);
}

/// With `--cwd` below the repository root, a relative preview path lands under `--cwd`, not at the root, for both
/// commands.
#[test]
fn a_relative_preview_path_is_under_cwd_when_cwd_is_below_the_workspace_root() {
    let dir = setup_polyglot_git_repo();
    let root_args = GlobalArgs {
        format: OutputFormat::Json,
        cwd: dir.path().to_path_buf(),
        dry_run: false,
    };
    init_repo(&root_args);
    commands::add::handle(
        AddArgs {
            packages: vec!["cargo/core-crate:minor".to_string()],
            summary: Some("Retry uploads.".to_string()),
        },
        &root_args,
    )
    .unwrap();
    let sub = dir.path().join("docs");
    std::fs::create_dir_all(&sub).unwrap();
    let global = GlobalArgs {
        cwd: sub.clone(),
        ..root_args
    };

    commands::version::handle(version_args(Some("preview.json".into())), &global).unwrap();

    assert!(sub.join("preview.json").exists(), "written under --cwd");
    assert!(!dir.path().join("preview.json").exists(), "not at the workspace root");
    let status = commands::compose_pr_body::handle(
        ComposePrBodyArgs {
            existing_body: None,
            branch: None,
            preview: Some("preview.json".into()),
            publish_plan: false,
        },
        &global,
    );
    assert_eq!(status.unwrap(), std::process::ExitCode::SUCCESS);
}

#[test]
fn a_plan_that_fails_writes_no_preview() {
    let dir = setup_polyglot_git_repo();
    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: dir.path().to_path_buf(),
        dry_run: false,
    };
    init_repo(&global);
    let path = dir.path().join("preview.json");
    std::fs::write(&path, "saved").unwrap();

    let mut args = version_args(Some(path.clone()));
    args.strict = true;
    let status = commands::version::handle(args, &global).unwrap();

    assert_eq!(
        status,
        std::process::ExitCode::FAILURE,
        "no pending changesets is an error under --strict"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "saved");
}

/// Planning the versioned tree's release, as `compose-pr-body --publish-plan` does, names where the package goes.
#[test]
fn the_publish_plan_of_a_versioned_tree_lists_the_crates_io_release() {
    let dir = setup_polyglot_git_repo();
    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: dir.path().to_path_buf(),
        dry_run: false,
    };
    init_repo(&global);
    commands::add::handle(
        AddArgs {
            packages: vec!["cargo/core-crate:minor".to_string()],
            summary: Some("Retry uploads.".to_string()),
        },
        &global,
    )
    .unwrap();
    commands::version::handle(version_args(Some("preview.json".into())), &global).unwrap();

    let stored = callisto_graph::release_preview::ReleasePreview::from_json(
        &std::fs::read_to_string(dir.path().join("preview.json")).unwrap(),
    )
    .unwrap();
    let selections: Vec<callisto_model::ReleasePackageId> = stored
        .release_selections()
        .expect("every bumped package is nameable")
        .iter()
        .map(|id| id.parse().unwrap())
        .collect();
    assert_eq!(selections.len(), 1);

    let runner = RecordingRunner::default();
    let locator = callisto_graph::locate::IgnoreWalkLocator::new(dir.path());
    let ws = callisto_graph::Workspace::load(dir.path().to_path_buf(), &locator, &runner).unwrap();
    let planned = callisto_graph::commands::plan_workspace_release_offline(&ws, &selections);
    let programs = runner.programs.lock().unwrap().clone();
    assert!(
        programs.iter().all(|program| program == "git"),
        "the offline plan ran something other than git: {programs:?}"
    );
    let outlook = callisto_graph::release_preview::publish_outlook(planned);
    let callisto_graph::release_preview::PublishOutlook::Planned { releases, .. } = &outlook else {
        panic!("expected a plan, got {outlook:?}");
    };
    let core = releases
        .iter()
        .find(|r| r.package == "core-crate")
        .expect("core-crate is released");
    assert_eq!(core.registries, ["crates.io"]);
    assert!(core.tag);
    let listed: Vec<&str> = releases.iter().map(|r| r.package.as_str()).collect();
    assert_eq!(
        listed,
        ["core-crate"],
        "an unreleased package with no changeset is not listed"
    );

    let preview = callisto_graph::release_preview::ReleasePreview::from_json(
        &std::fs::read_to_string(dir.path().join("preview.json")).unwrap(),
    )
    .unwrap()
    .with_publish(outlook);
    let body = callisto_graph::commands::render_pr_body_from_preview(&preview, &PrBodyOptions::default()).body;
    assert!(body.contains("Merging will publish"), "{body}");
    assert!(body.contains("`core-crate`"), "{body}");

    let status = commands::compose_pr_body::handle(
        ComposePrBodyArgs {
            existing_body: None,
            branch: None,
            preview: Some("preview.json".into()),
            publish_plan: true,
        },
        &global,
    );
    assert_eq!(status.unwrap(), std::process::ExitCode::SUCCESS);
}

/// Delegates to the real runner and remembers which programs were started.
#[derive(Default)]
struct RecordingRunner {
    programs: std::sync::Mutex<Vec<String>>,
}

impl callisto_model::CommandRunner for RecordingRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        cwd: &std::path::Path,
    ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
        self.programs.lock().unwrap().push(program.to_owned());
        callisto_cli::runner::CliCommandRunner.run(program, args, cwd)
    }
}

/// `version --emit-preview` records the publishable packages whose source changed and that the plan leaves out, and the
/// body leads with them; the package the changeset bumps is not among them.
#[test]
fn the_preview_lists_a_publishable_package_the_plan_leaves_out() {
    let dir = setup_polyglot_git_repo();
    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: dir.path().to_path_buf(),
        dry_run: false,
    };
    init_repo(&global);
    commands::add::handle(
        AddArgs {
            packages: vec!["cargo/core-crate:minor".to_string()],
            summary: Some("Retry uploads.\n\nAction: re-run the installer.".to_string()),
        },
        &global,
    )
    .unwrap();

    commands::version::handle(version_args(Some("preview.json".into())), &global).unwrap();

    let preview = callisto_graph::release_preview::ReleasePreview::from_json(
        &std::fs::read_to_string(dir.path().join("preview.json")).unwrap(),
    )
    .unwrap();
    let names: Vec<&str> = preview
        .unreleased
        .iter()
        .map(|change| change.package.as_str())
        .collect();
    assert!(names.contains(&"@myorg/web-app"), "{names:?}");
    assert!(!names.contains(&"core-crate"), "{names:?}");

    let body = callisto_graph::commands::render_pr_body_from_preview(&preview, &PrBodyOptions::default()).body;
    assert!(body.contains("**Read before merging**"), "{body}");
    assert!(
        body.contains("**Action required** (`core-crate`): re-run the installer."),
        "{body}"
    );
    assert!(body.contains("**Changed, not releasing** (`@myorg/web-app`"), "{body}");
}

/// A private member of a fixed group is bumped and released with its group but has no publish target. Selecting it
/// for the offline plan used to fail with E169 and put a false "would fail" caution on a healthy release PR.
#[test]
fn a_private_fixed_group_member_does_not_make_the_publish_plan_fail() {
    let dir = setup_polyglot_git_repo();
    let root = dir.path();
    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: root.to_path_buf(),
        dry_run: false,
    };
    init_repo(&global);
    std::fs::create_dir_all(root.join("crates/private/src")).unwrap();
    std::fs::write(
        root.join("crates/private/Cargo.toml"),
        "[package]\nname = \"private-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n",
    )
    .unwrap();
    std::fs::write(root.join("crates/private/src/lib.rs"), "\n").unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/core\", \"crates/private\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    let config = std::fs::read_to_string(root.join("callisto.toml")).unwrap();
    std::fs::write(
        root.join("callisto.toml"),
        format!(
            "{config}\n[[fixed-group]]\nname = \"lockstep\"\nmembers = [\"cargo/core-crate\", \"cargo/private-crate\"]\n"
        ),
    )
    .unwrap();
    for args in [vec!["add", "."], vec!["commit", "-q", "-m", "private member"]] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }
    commands::add::handle(
        AddArgs {
            packages: vec!["cargo/core-crate:minor".to_string()],
            summary: Some("Retry uploads.".to_string()),
        },
        &global,
    )
    .unwrap();
    commands::version::handle(version_args(Some("preview.json".into())), &global).unwrap();

    let preview = callisto_graph::release_preview::ReleasePreview::from_json(
        &std::fs::read_to_string(root.join("preview.json")).unwrap(),
    )
    .unwrap();
    let names: Vec<&str> = preview.packages.iter().map(|p| p.name.as_str()).collect();
    assert!(
        names.contains(&"private-crate"),
        "the private member is bumped with its group: {names:?}"
    );

    let runner = callisto_cli::runner::CliCommandRunner;
    let ws = callisto_cli::workspace::load_workspace(&global, &runner).unwrap();
    let outlook =
        callisto_graph::release_preview::publish_outlook_in(&ws, &preview).expect("every package is nameable");
    let callisto_graph::release_preview::PublishOutlook::Planned { releases, .. } = &outlook else {
        panic!("a healthy release must not be blocked: {outlook:?}");
    };
    let listed: Vec<&str> = releases.iter().map(|r| r.package.as_str()).collect();
    assert_eq!(listed, ["core-crate"]);
}

#[test]
fn emit_preview_is_refused_with_dry_run_before_anything_is_written() {
    let dir = setup_polyglot_git_repo();
    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: dir.path().to_path_buf(),
        dry_run: true,
    };
    let path = dir.path().join("preview.json");
    let error = commands::version::handle(version_args(Some(path.clone())), &global).unwrap_err();
    assert!(error.to_string().contains("--emit-preview writes a file"), "{error}");
    assert!(!path.exists());
}

#[test]
fn a_preview_for_another_schema_version_is_refused() {
    let dir = setup_polyglot_git_repo();
    let global = GlobalArgs {
        format: OutputFormat::Json,
        cwd: dir.path().to_path_buf(),
        dry_run: false,
    };
    let path = dir.path().join("preview.json");
    std::fs::write(
        &path,
        r#"{"schemaVersion":99,"packages":[],"changes":[],"bystanders":[],"findings":[]}"#,
    )
    .unwrap();
    let error = commands::compose_pr_body::handle(
        ComposePrBodyArgs {
            existing_body: None,
            branch: None,
            preview: Some(path),
            publish_plan: false,
        },
        &global,
    )
    .unwrap_err();
    assert!(error.to_string().contains("schemaVersion 99"), "{error}");
}
