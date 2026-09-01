//! Read-only durable release interfaces.
//!
//! Planning deliberately accepts only exact package identities. It never
//! accepts an inline intent or searches for an authority file. Execution is
//! intentionally not wired until the provider adapter can prove exact remote
//! identities; exposing a permissive fallback here would recreate the legacy
//! publish/tag bypass this command family replaces.

use std::{io::Write, process::ExitCode};

use callisto_graph::commands::{
    build_release_intent, derive_selected_release_decision, reconcile_release_execution, ReleaseStateStore,
    VersionOptions,
};
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_model::{ExecutionTrustProfileV1, ReleaseExecutionStateV1, ReleaseIntentV1, ReleasePackageId};

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
        ReleaseArgs::Execute(args) => execute(args),
    }
}

fn plan(args: ReleasePlanArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    if global.dry_run {
        return Err(CliError::Other(
            "release plan needs an output file; remove --dry-run because planning is already read-only".to_string(),
        ));
    }
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
    let runner = CliCommandRunner;
    let workspace = load_workspace(global, &runner)?;
    let inference = select_inference();
    let version_plan = callisto_graph::commands::plan_version(&workspace, &inference, &VersionOptions::default())?;
    let decision = derive_selected_release_decision(&workspace, &version_plan, &selections)?;
    let locator = IgnoreWalkLocator::new(&workspace.root);
    let intent = build_release_intent(
        &workspace.root,
        &locator,
        &runner,
        &decision,
        ExecutionTrustProfileV1::GitCommit,
    )?;
    write_intent(&args.out, &intent)?;
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

fn execute(args: ReleaseExecuteArgs) -> Result<ExitCode, CliError> {
    let _intent = read_intent(&args.intent)?;
    let _manifest = args.artifact_manifest.map(|path| read_json_file(&path)).transpose()?;
    let _state = args.state;
    Err(CliError::Other(
        "release execute is not enabled until Callisto's provider adapters can verify exact registry, Git, forge, and GitHub attestation identities; use release inspect or release reconcile while migrating from the legacy workflow".to_string(),
    ))
}

fn read_intent(path: &std::path::Path) -> Result<ReleaseIntentV1, CliError> {
    serde_json::from_value(read_json_file(path)?)
        .map_err(|error| CliError::Other(format!("invalid release intent {}: {error}", path.display())))
}

fn read_json_file(path: &std::path::Path) -> Result<serde_json::Value, CliError> {
    let content = std::fs::read_to_string(path).map_err(|source| CliError::Io {
        source,
        path: Some(path.to_path_buf()),
    })?;
    serde_json::from_str(&content)
        .map_err(|error| CliError::Other(format!("invalid JSON in {}: {error}", path.display())))
}

fn write_intent(path: &std::path::Path, intent: &ReleaseIntentV1) -> Result<(), CliError> {
    let content = serde_json::to_string_pretty(intent).expect("release intent serializes") + "\n";
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| CliError::Io {
        source,
        path: Some(parent.to_path_buf()),
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|source| CliError::Io {
        source,
        path: Some(parent.to_path_buf()),
    })?;
    temporary.write_all(content.as_bytes()).map_err(|source| CliError::Io {
        source,
        path: Some(path.to_path_buf()),
    })?;
    temporary.as_file().sync_all().map_err(|source| CliError::Io {
        source,
        path: Some(path.to_path_buf()),
    })?;
    temporary.persist(path).map_err(|error| CliError::Io {
        source: error.error,
        path: Some(path.to_path_buf()),
    })?;
    Ok(())
}
