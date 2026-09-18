//! Read-only durable release interfaces.
//!
//! Planning deliberately accepts only exact package identities. It never
//! accepts an inline intent or searches for an authority file. Execution is
//! intentionally not wired until the provider adapter can prove exact remote
//! identities; exposing a permissive fallback here would recreate the legacy
//! publish/tag bypass this command family replaces.

use std::process::ExitCode;

use callisto_graph::commands::{
    build_release_intent, build_release_intent_with_artifacts, derive_release_commit_decision,
    derive_selected_release_decision, execute_release_with_artifacts_in_recovery,
    observe_release_operations_with_artifacts, reconcile_release_execution,
    validate_release_intent_with_state_directory, verify_artifact_manifest, ReleaseStateStore, VersionOptions,
};
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_model::{
    ApplyPermit, ArtifactDigest, ArtifactManifestEntryV1, ArtifactManifestV1, ExecutionTrustProfileV1,
    GitHubArtifactAttestationV1, ReleaseExecutionStateV1, ReleaseIntentV1, ReleasePackageId, ReleaseProfileId,
    ReleaseReceiptV1, ReleaseRunKindV1, ReleaseRunProvenanceV1,
};

use crate::cli::{
    GlobalArgs, OutputFormat, ReleaseArgs, ReleaseArtifactManifestArgs, ReleaseExecuteArgs, ReleaseInspectArgs,
    ReleasePlanArgs, ReleaseReconcileArgs,
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
        ReleaseArgs::ArtifactManifest(args) => artifact_manifest(args, global),
        ReleaseArgs::Execute(args) => execute(args, global),
    }
}

fn artifact_manifest(args: ReleaseArtifactManifestArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    if global.dry_run {
        return Err(CliError::Other(
            "release artifact-manifest needs an output file; remove --dry-run because it records build output"
                .to_owned(),
        ));
    }
    let intent = read_intent(&args.intent)?;
    if intent.artifact_slots.is_empty() {
        return Err(CliError::Other(
            "release intent declares no binary artifact slots; no artifact manifest can be created".to_owned(),
        ));
    }
    let root = args.artifact_dir.canonicalize().map_err(|source| CliError::Io {
        source,
        path: Some(args.artifact_dir.clone()),
    })?;
    if !matches!(intent.snapshot.source, callisto_model::SourceIdentity::GitCommit { .. }) {
        return Err(CliError::Other(
            "artifact manifests require a Git commit release source".to_owned(),
        ));
    }
    let mut entries = Vec::with_capacity(intent.artifact_slots.len());
    for slot in &intent.artifact_slots {
        let path = root.join(&slot.asset_name);
        let metadata = std::fs::symlink_metadata(&path).map_err(|source| CliError::Io {
            source,
            path: Some(path.clone()),
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CliError::Other(format!(
                "artifact `{}` must be a regular file directly in `{}`",
                slot.asset_name,
                root.display()
            )));
        }
        let canonical = path.canonicalize().map_err(|source| CliError::Io {
            source,
            path: Some(path.clone()),
        })?;
        if !canonical.starts_with(&root) {
            return Err(CliError::Other(format!(
                "artifact `{}` resolves outside `{}`",
                slot.asset_name,
                root.display()
            )));
        }
        let file = std::fs::File::open(&canonical).map_err(|source| CliError::Io {
            source,
            path: Some(canonical.clone()),
        })?;
        let (digest, byte_length) = ArtifactDigest::from_reader(file).map_err(|source| CliError::Io {
            source,
            path: Some(canonical),
        })?;
        entries.push(ArtifactManifestEntryV1 {
            slot: slot.clone(),
            digest: digest.clone(),
            byte_length,
            attestation: GitHubArtifactAttestationV1 {
                repository: slot.attestation_policy.repository.clone(),
                workflow_path: slot.attestation_policy.workflow_path.clone(),
                workflow_commit: slot.attestation_policy.workflow_commit.clone(),
                subject_digest: digest,
                source_commit: slot.attestation_policy.workflow_commit.clone(),
            },
        });
    }
    let manifest = ArtifactManifestV1::new(&intent, entries)
        .map_err(|error| CliError::Other(format!("cannot create artifact manifest: {error}")))?;
    let permit = ApplyPermit::granted_unless_dry_run(false).expect("non-dry-run permits writes");
    let content = serde_json::to_string_pretty(&manifest).expect("artifact manifest serializes") + "\n";
    callisto_model::atomic::atomic_write(&args.out, &content, &permit).map_err(|source| CliError::Io {
        source,
        path: Some(args.out.clone()),
    })?;
    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &manifest)?,
        OutputFormat::Text => println!("Artifact manifest saved to {}", args.out.display()),
    }
    Ok(ExitCode::SUCCESS)
}

fn plan(args: ReleasePlanArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    if global.dry_run {
        return Err(CliError::Other(
            "release plan needs an output file; remove --dry-run because planning is already read-only".to_string(),
        ));
    }
    let runner = CliCommandRunner;
    let profile = ReleaseProfileId::parse(&args.profile)
        .map_err(|error| CliError::Other(format!("invalid release profile `{}`: {error}", args.profile)))?;
    let source_global = source_global(global, args.source_root.as_deref());
    let workspace = load_workspace(&source_global, &runner)?;
    let decision = match args.from_release_commit.as_deref() {
        Some(raw) => {
            let commit = callisto_model::CommitSha::parse(raw)
                .map_err(|error| CliError::Other(format!("invalid merged release commit `{raw}`: {error}")))?;
            let decision_path = args
                .decision
                .as_deref()
                .expect("clap requires --decision alongside --from-release-commit");
            // The workflow intentionally passes an absolute decision path
            // from its separately checked-out release source. Git's commit
            // diff and `git show REV:path` both require a repository-relative
            // path, so normalize once at the CLI boundary rather than asking
            // graph code to infer a caller's checkout layout.
            let decision_path = if decision_path.is_absolute() {
                let decision_path = dunce::canonicalize(decision_path).map_err(|source| CliError::Io {
                    source,
                    path: Some(decision_path.to_path_buf()),
                })?;
                decision_path
                    .strip_prefix(&workspace.root)
                    .map_err(|_outside_source| {
                        CliError::Other(format!(
                            "release decision {} must be inside selected source root {}",
                            decision_path.display(),
                            workspace.root.display()
                        ))
                    })?
                    .to_path_buf()
            } else {
                decision_path.to_path_buf()
            };
            let decision_path = callisto_model::workspace_relative(&decision_path).map_err(|error| {
                CliError::Other(format!(
                    "invalid release decision path {}: {error}",
                    decision_path.display()
                ))
            })?;
            derive_release_commit_decision(&workspace, &commit, &decision_path)?
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
    let intent = match (
        &workspace.config.product_release,
        &args.orchestration_revision,
        &args.artifact_repository,
    ) {
        (Some(_), Some(revision), Some(repository)) => {
            let workflow_commit = callisto_model::CommitSha::parse(revision)
                .map_err(|error| CliError::Other(format!("invalid orchestration revision `{revision}`: {error}")))?;
            let repository = callisto_model::GitHubRepository::parse(repository)
                .map_err(|error| CliError::Other(format!("invalid artifact repository `{repository}`: {error}")))?;
            build_release_intent_with_artifacts(
                &workspace.root,
                &locator,
                &runner,
                &decision,
                profile.clone(),
                ExecutionTrustProfileV1::GitCommit,
                callisto_graph::commands::ArtifactBuildPolicy {
                    repository,
                    workflow_path: ".github/workflows/callisto-release.yml".to_owned(),
                    workflow_commit,
                },
            )?
        }
        (Some(_), _, _) => {
            return Err(CliError::Other(
                "product release planning requires --orchestration-revision and --artifact-repository".to_owned(),
            ));
        }
        (None, None, None) => build_release_intent(
            &workspace.root,
            &locator,
            &runner,
            &decision,
            profile,
            ExecutionTrustProfileV1::GitCommit,
        )?,
        (None, _, _) => {
            return Err(CliError::Other(
                "--orchestration-revision and --artifact-repository require a configured product release".to_owned(),
            ));
        }
    };
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
    let selected_profile = ReleaseProfileId::parse(&args.profile)
        .map_err(|error| CliError::Other(format!("invalid release profile `{}`: {error}", args.profile)))?;
    if selected_profile != intent.profile {
        return Err(CliError::Other(format!(
            "release execute profile `{}` does not match immutable intent profile `{}`",
            selected_profile.as_str(),
            intent.profile.as_str()
        )));
    }
    let manifest = args
        .artifact_manifest
        .as_deref()
        .map(read_artifact_manifest)
        .transpose()?;
    let verified_artifacts = match (
        intent.artifact_slots.is_empty(),
        manifest.as_ref(),
        args.artifact_dir.as_deref(),
    ) {
        (true, None, None) => None,
        (true, _, _) => {
            return Err(CliError::Other(
                "release intent declares no binary artifact slots; omit --artifact-manifest and --artifact-dir"
                    .to_owned(),
            ));
        }
        (false, Some(manifest), Some(directory)) => {
            let runner = CliCommandRunner;
            Some(verify_artifact_manifest(&intent, manifest, directory, &runner)?)
        }
        (false, _, _) => {
            return Err(CliError::Other(
                "release intent declares binary artifact slots; provide both --artifact-manifest and --artifact-dir"
                    .to_owned(),
            ));
        }
    };
    let runner = CliCommandRunner;
    let source_global = source_global(global, args.source_root.as_deref());
    let root = dunce::canonicalize(&source_global.cwd).map_err(|source| CliError::Io {
        source,
        path: Some(source_global.cwd.clone()),
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
    let state = execute_release_with_artifacts_in_recovery(
        &capability,
        &store,
        &permit,
        verified_artifacts.as_ref(),
        args.recovery,
    )?;
    let orchestration_revision = callisto_model::CommitSha::parse(&args.orchestration_revision).map_err(|error| {
        CliError::Other(format!(
            "invalid orchestration revision `{}`: {error}",
            args.orchestration_revision
        ))
    })?;
    let source = match &capability.intent().snapshot.source {
        callisto_model::SourceIdentity::GitCommit { sha } => sha.clone(),
        callisto_model::SourceIdentity::HermeticContent { .. } => {
            return Err(CliError::Other(
                "release receipts require a Git commit release source".to_owned(),
            ))
        }
    };
    let mut provenance = ReleaseRunProvenanceV1::new(
        if args.recovery {
            ReleaseRunKindV1::Recovery
        } else {
            ReleaseRunKindV1::Initial
        },
        orchestration_revision,
        source,
        selected_profile,
        capability.intent().digest().clone(),
    );
    if let Some(artifacts) = verified_artifacts.as_ref() {
        provenance = provenance.with_artifact_manifest_digest(artifacts.manifest().digest());
    }
    let receipt = ReleaseReceiptV1::from_evidence(
        capability.intent(),
        &state,
        provenance,
        observe_release_operations_with_artifacts(&capability, verified_artifacts.as_ref())?,
    )
    .map_err(|error| CliError::Other(format!("cannot issue terminal release receipt: {error}")))?;
    write_receipt(&args.receipt, &receipt, &permit)?;
    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &receipt)?,
        OutputFormat::Text => println!(
            "Release receipt saved to {} (execution state: {})",
            args.receipt.display(),
            store.path().display()
        ),
    }
    Ok(ExitCode::SUCCESS)
}

fn source_global(global: &GlobalArgs, source_root: Option<&std::path::Path>) -> GlobalArgs {
    let mut source = global.clone();
    if let Some(root) = source_root {
        source.cwd = root.to_path_buf();
    }
    source
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

fn write_receipt(path: &std::path::Path, receipt: &ReleaseReceiptV1, permit: &ApplyPermit) -> Result<(), CliError> {
    let content = serde_json::to_string_pretty(receipt).expect("release receipt serializes") + "\n";
    callisto_model::atomic::atomic_write(path, &content, permit).map_err(|source| CliError::Io {
        source,
        path: Some(path.to_path_buf()),
    })
}
