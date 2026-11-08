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

/// `version` and `compose-pr-body` must agree on what a relative preview path means.
#[test]
fn a_relative_preview_path_means_the_workspace_root_for_both_commands() {
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
