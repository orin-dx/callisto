//! Read-only durable release interfaces.
//!
//! Planning deliberately accepts only exact package identities. It never
//! accepts an inline intent or searches for an authority file. Execution is
//! intentionally not wired until the provider adapter can prove exact remote
//! identities; exposing a permissive fallback here would recreate the legacy
//! publish/tag bypass this command family replaces.

use std::process::ExitCode;

use callisto_graph::commands::{
    build_release_intent, derive_release_commit_decision, derive_selected_release_decision,
    execute_release_with_artifacts, reconcile_release_execution, validate_release_intent_with_state_directory,
    verify_artifact_manifest, PreparedReleaseEffectAdapter, ReleaseStateStore, VersionOptions,
};
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_model::{
    ApplyPermit, ArtifactManifestV1, ExecutionTrustProfileV1, ReleaseExecutionStateV1, ReleaseIntentV1,
    ReleasePackageId,
};

use crate::cli::{
    GlobalArgs, OutputFormat, ReleaseArgs, ReleaseExecuteArgs, ReleaseInspectArgs, ReleasePlanArgs,
    ReleaseReconcileArgs,
};
use crate::error::CliError;
use crate::output::write_json;
use crate::runner::CliCommandRunner;
use crate::workspace::{load_workspace, select_inference};

pub fn handle(args: ReleaseArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    match args {
        ReleaseArgs::Plan(args) => plan(args, global),
        ReleaseArgs::Inspect(args) => inspect(args, global),
        ReleaseArgs::Reconcile(args) => reconcile(args, global),
        ReleaseArgs::Execute(args) => execute(args, global),
    }
}

fn plan(args: ReleasePlanArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    if global.dry_run {
        return Err(CliError::Other(
            "release plan needs an output file; remove --dry-run because planning is already read-only".to_string(),
        ));
    }
    let runner = CliCommandRunner;
    let workspace = load_workspace(global, &runner)?;
    let decision = match args.from_release_commit.as_deref() {
        Some(raw) => {
            let commit = callisto_model::CommitSha::parse(raw)
                .map_err(|error| CliError::Other(format!("invalid merged release commit `{raw}`: {error}")))?;
            derive_release_commit_decision(&workspace, &commit)?
        }
        None => {
            let selections = args
                .packages
                .iter()
                .map(|raw| {
                    ReleasePackageId::parse(raw).map_err(|error| {
                        CliError::Other(format!(
                            "invalid release package `{raw}`: {error}; use an exact ecosystem-qualified identity such as cargo/callisto-cli"
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let inference = select_inference();
            let version_plan =
                callisto_graph::commands::plan_version(&workspace, &inference, &VersionOptions::default())?;
            derive_selected_release_decision(&workspace, &version_plan, &selections)?
        }
    };
    let locator = IgnoreWalkLocator::new(&workspace.root);
    let intent = build_release_intent(
        &workspace.root,
        &locator,
        &runner,
        &decision,
        ExecutionTrustProfileV1::GitCommit,
    )?;
    let permit = ApplyPermit::granted_unless_dry_run(global.dry_run)
        .expect("release plan rejects --dry-run before creating its explicit output");
    write_intent(&args.out, &intent, &permit)?;
    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &intent)?,
        OutputFormat::Text => println!("Wrote release intent {} to {}", intent.digest(), args.out.display()),
    }
    Ok(ExitCode::SUCCESS)
}

fn inspect(args: ReleaseInspectArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let value = read_json_file(&args.input)?;
    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &value)?,
        OutputFormat::Text => println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("JSON value serializes")
        ),
    }
    Ok(ExitCode::SUCCESS)
}

fn reconcile(args: ReleaseReconcileArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let intent = read_intent(&args.intent)?;
    let state = match args.state {
        Some(path) => ReleaseStateStore::new(path).load(&intent)?,
        None => None,
    }
    .unwrap_or_else(|| ReleaseExecutionStateV1::pending(&intent));
    let report = reconcile_release_execution(&intent, &state)?;
    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), report.eligible())?,
        OutputFormat::Text => {
            for operation in report.eligible() {
                println!("eligible: {operation:?}");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn execute(args: ReleaseExecuteArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let intent = read_intent(&args.intent)?;
    let manifest = args
        .artifact_manifest
        .as_deref()
        .map(read_artifact_manifest)
        .transpose()?;
    match (
        intent.artifact_slots.is_empty(),
        manifest.as_ref(),
        args.artifact_dir.as_deref(),
    ) {
        (true, None, None) => {}
        (true, _, _) => {
            return Err(CliError::Other(
                "release intent declares no binary artifact slots; omit --artifact-manifest and --artifact-dir"
                    .to_owned(),
            ));
        }
        (false, Some(manifest), Some(directory)) => {
            let runner = CliCommandRunner;
            verify_artifact_manifest(&intent, manifest, directory, &runner)?;
        }
        (false, _, _) => {
            return Err(CliError::Other(
                "release intent declares binary artifact slots; provide both --artifact-manifest and --artifact-dir"
                    .to_owned(),
            ));
        }
    }
    let runner = CliCommandRunner;
    let root = dunce::canonicalize(&global.cwd).map_err(|source| CliError::Io {
        source,
        path: Some(global.cwd.clone()),
    })?;
    let locator = IgnoreWalkLocator::new(&root);
    let explicit_state_directory = args.state.as_deref().and_then(std::path::Path::parent);
    let capability =
        validate_release_intent_with_state_directory(&root, &locator, &runner, explicit_state_directory, intent)?;
    let store = match args.state {
        Some(path) => ReleaseStateStore::new(path),
        None => ReleaseStateStore::default_for(&root, capability.intent())?,
    };
    let permit = ApplyPermit::granted_unless_dry_run(global.dry_run).ok_or_else(|| {
        CliError::Other(
            "release execute cannot run with --dry-run; use release reconcile for a read-only readiness check"
                .to_string(),
        )
    })?;
    let mut adapter = PreparedReleaseEffectAdapter;
    let state = execute_release_with_artifacts(&capability, &store, &permit, manifest.as_ref(), &mut adapter)?;
    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &state)?,
        OutputFormat::Text => println!("Release execution state saved to {}", store.path().display()),
    }
    Ok(ExitCode::SUCCESS)
}

fn read_intent(path: &std::path::Path) -> Result<ReleaseIntentV1, CliError> {
    serde_json::from_value(read_json_file(path)?)
        .map_err(|error| CliError::Other(format!("invalid release intent {}: {error}", path.display())))
}

fn read_artifact_manifest(path: &std::path::Path) -> Result<ArtifactManifestV1, CliError> {
    serde_json::from_value(read_json_file(path)?)
        .map_err(|error| CliError::Other(format!("invalid artifact manifest {}: {error}", path.display())))
}

fn read_json_file(path: &std::path::Path) -> Result<serde_json::Value, CliError> {
    let content = std::fs::read_to_string(path).map_err(|source| CliError::Io {
        source,
        path: Some(path.to_path_buf()),
    })?;
    serde_json::from_str(&content)
        .map_err(|error| CliError::Other(format!("invalid JSON in {}: {error}", path.display())))
}

fn write_intent(path: &std::path::Path, intent: &ReleaseIntentV1, permit: &ApplyPermit) -> Result<(), CliError> {
    let content = serde_json::to_string_pretty(intent).expect("release intent serializes") + "\n";
    callisto_model::atomic::atomic_write(path, &content, permit).map_err(|source| CliError::Io {
        source,
        path: Some(path.to_path_buf()),
    })?;
    Ok(())
}
