//! Durable release interfaces (plan and execute).
//!
//! Planning accepts only exact package identities. It never accepts an inline
//! intent or searches for an authority file. `execute` is the only durable
//! mutation route and requires an explicit intent, receipt, and orchestration
//! revision; there is no permissive fallback to the legacy publish/tag path.

use std::process::ExitCode;

use callisto_graph::commands::{
    build_release_intent, build_release_intent_with_artifacts, derive_release_commit_decision,
    derive_selected_release_decision, execute_release, reconcile_release_execution,
    validate_release_intent_with_state_directory, verify_artifact_manifest, ReleaseStateStore, VersionOptions,
};
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_model::{
    ApplyPermit, ArtifactDigest, ArtifactManifestEntryV1, ArtifactManifestV1, ExecutionTrustProfileV1,
    GitHubArtifactAttestationV1, ReleaseIntentV1, ReleasePackageId, ReleaseProfileId, ReleaseReceiptV1,
    ReleaseRunEnvelopeV1, ReleaseRunKindV1,
};

use crate::cli::{
    GlobalArgs, OutputFormat, ReleaseArgs, ReleaseArtifactManifestArgs, ReleaseExecuteArgs, ReleaseInspectArgs,
    ReleasePlanArgs, ReleaseReconcileArgs,
};
use crate::error::CliError;
use crate::output::{log_line, write_json};
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
        return Err(CliError::ReleaseArtifactManifestDryRun);
    }
    let intent = read_intent(&args.intent)?;
    if intent.artifact_slots.is_empty() {
        return Err(CliError::ReleaseNoArtifactSlots);
    }
    let root = args.artifact_dir.canonicalize().map_err(|source| CliError::Io {
        source,
        path: Some(args.artifact_dir.clone()),
    })?;
    if !matches!(intent.snapshot.source, callisto_model::SourceIdentity::GitCommit { .. }) {
        return Err(CliError::ReleaseManifestSourceNotGit);
    }
    let mut entries = Vec::with_capacity(intent.artifact_slots.len());
    for slot in &intent.artifact_slots {
        let path = root.join(&slot.asset_name);
        let metadata = std::fs::symlink_metadata(&path).map_err(|source| CliError::Io {
            source,
            path: Some(path.clone()),
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CliError::ReleaseArtifactNotRegularFile {
                asset: slot.asset_name.to_string(),
                dir: root.display().to_string(),
            });
        }
        let canonical = path.canonicalize().map_err(|source| CliError::Io {
            source,
            path: Some(path.clone()),
        })?;
        if !canonical.starts_with(&root) {
            return Err(CliError::ReleaseArtifactEscapesDirectory {
                asset: slot.asset_name.to_string(),
                dir: root.display().to_string(),
            });
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
    let manifest = ArtifactManifestV1::new(&intent, entries).map_err(|error| CliError::ReleaseManifestInvalid {
        detail: error.to_string(),
    })?;
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
        return Err(CliError::ReleasePlanDryRun);
    }
    let runner = CliCommandRunner;
    let profile = ReleaseProfileId::parse(&args.profile).map_err(|error| CliError::ReleaseProfileInvalid {
        profile: args.profile.clone(),
        detail: error.to_string(),
    })?;
    let source_global = source_global(global, args.source_root.as_deref());
    let workspace = load_workspace(&source_global, &runner)?;
    let decision = match args.from_release_commit.as_deref() {
        Some(raw) => {
            let commit = callisto_model::CommitSha::parse(raw).map_err(|error| CliError::ReleaseCommitInvalid {
                raw: raw.to_owned(),
                detail: error.to_string(),
            })?;
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
                    .map_err(|_outside_source| CliError::ReleaseDecisionOutsideSource {
                        decision: decision_path.display().to_string(),
                        root: workspace.root.display().to_string(),
                    })?
                    .to_path_buf()
            } else {
                decision_path.to_path_buf()
            };
            let decision_path = callisto_model::workspace_relative(&decision_path).map_err(|error| {
                CliError::ReleaseDecisionPathInvalid {
                    path: decision_path.display().to_string(),
                    detail: error.to_string(),
                }
            })?;
            derive_release_commit_decision(&workspace, &commit, &decision_path)?
        }
        None => {
            let selections = args
                .packages
                .iter()
                .map(|raw| {
                    ReleasePackageId::parse(raw).map_err(|error| CliError::ReleasePackageInvalid {
                        raw: raw.clone(),
                        detail: error.to_string(),
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
            let workflow_commit = callisto_model::CommitSha::parse(revision).map_err(|error| {
                CliError::ReleaseOrchestrationRevisionInvalid {
                    revision: revision.clone(),
                    detail: error.to_string(),
                }
            })?;
            let repository = callisto_model::GitHubRepository::parse(repository).map_err(|error| {
                CliError::ReleaseArtifactRepositoryInvalid {
                    repository: repository.clone(),
                    detail: error.to_string(),
                }
            })?;
            // Profile existence is validated by the graph; only the destination match lives here.
            if let Some(configured) = workspace
                .config
                .product_release
                .as_ref()
                .and_then(|release| release.profile(&profile))
                .filter(|configured| configured.forge_repository != repository)
            {
                return Err(CliError::ReleaseProfileRepositoryMismatch {
                    profile: profile.as_str().to_owned(),
                    configured: configured.forge_repository.as_slug().to_string(),
                    requested: repository.as_slug().to_string(),
                });
            }
            build_release_intent_with_artifacts(
                &workspace.root,
                &locator,
                &runner,
                &decision,
                profile.clone(),
                ExecutionTrustProfileV1::GitCommit,
                callisto_graph::commands::ArtifactBuildPolicy {
                    repository,
                    workflow_path: callisto_model::RELEASE_COORDINATOR_WORKFLOW_PATH.to_owned(),
                    workflow_commit,
                },
            )?
        }
        (Some(_), _, _) => {
            return Err(CliError::ReleaseOrchestrationFlagsRequired);
        }
        (None, revision, repository) => {
            // The release workflow always passes both flags, including for sources predating [release].
            if revision.is_some() || repository.is_some() {
                log_line(
                    global.format,
                    "notice: --orchestration-revision and --artifact-repository ignored: the source has no [release] section; planning with zero artifact slots",
                );
            }
            build_release_intent(
                &workspace.root,
                &locator,
                &runner,
                &decision,
                profile,
                ExecutionTrustProfileV1::GitCommit,
            )?
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
        Some(path) => ReleaseStateStore::new(path).load_for_intent(&intent)?,
        None => None,
    };
    let report = reconcile_release_execution(&intent, state.as_ref())?;
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
    // Every input is validated and the receipt destination probed before the first effect, so a
    // late failure cannot leave published crates, tags or releases without a receipt.
    let permit = ApplyPermit::granted_unless_dry_run(global.dry_run).ok_or(CliError::ReleaseExecuteDryRun)?;
    let intent = read_intent(&args.intent)?;
    let selected_profile = ReleaseProfileId::parse(&args.profile).map_err(|error| CliError::ReleaseProfileInvalid {
        profile: args.profile.clone(),
        detail: error.to_string(),
    })?;
    if selected_profile != intent.profile {
        return Err(CliError::ReleaseProfileMismatch {
            selected: selected_profile.as_str().to_owned(),
            intent: intent.profile.as_str().to_owned(),
        });
    }
    let orchestration_revision = callisto_model::CommitSha::parse(&args.orchestration_revision).map_err(|error| {
        CliError::ReleaseOrchestrationRevisionInvalid {
            revision: args.orchestration_revision.clone(),
            detail: error.to_string(),
        }
    })?;
    callisto_model::atomic::probe_atomic_write(&args.receipt, &permit).map_err(|source| CliError::Io {
        source,
        path: Some(args.receipt.clone()),
    })?;
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
            return Err(CliError::ReleaseUnexpectedArtifactInputs);
        }
        (false, Some(manifest), Some(directory)) => {
            let runner = CliCommandRunner;
            Some(verify_artifact_manifest(&intent, manifest, directory, &runner)?)
        }
        (false, _, _) => {
            return Err(CliError::ReleaseMissingArtifactInputs);
        }
    };
    // The run envelope is minted and cross-checked here, before the first
    // effect, and is then the single authority for every fact it carries.
    let envelope = ReleaseRunEnvelopeV1::new(
        if args.recovery {
            ReleaseRunKindV1::Recovery
        } else {
            ReleaseRunKindV1::Initial
        },
        orchestration_revision,
        &intent,
        verified_artifacts
            .as_ref()
            .map(|artifacts| artifacts.manifest().digest()),
    )
    .map_err(|error| CliError::ReleaseEnvelopeInvalid {
        detail: error.to_string(),
    })?;
    let runner = CliCommandRunner;
    let source_global = source_global(global, args.source_root.as_deref());
    let source_workspace = load_workspace(&source_global, &runner)?;
    // Profile existence is validated by the graph during intent validation; only the destination match lives here.
    if let Some(configured_profile) = source_workspace
        .config
        .product_release
        .as_ref()
        .and_then(|release| release.profile(&selected_profile))
    {
        if intent
            .artifact_slots
            .iter()
            .any(|slot| slot.attestation_policy.repository != configured_profile.forge_repository)
        {
            return Err(CliError::ReleaseArtifactDestinationMismatch {
                profile: selected_profile.as_str().to_owned(),
                repository: configured_profile.forge_repository.as_slug().to_string(),
            });
        }
    }
    // The workspace root, resolved exactly as `plan` resolves it. Executing
    // from a subdirectory must reach the same root, not the process cwd.
    let root = source_workspace.root.clone();
    let locator = IgnoreWalkLocator::new(&root);
    let explicit_state_directory = args.state.as_deref().map(state_directory_of);
    let capability =
        validate_release_intent_with_state_directory(&root, &locator, &runner, explicit_state_directory, intent)?;
    let store = match args.state {
        Some(path) => ReleaseStateStore::new(path),
        None => ReleaseStateStore::default_for(&root, capability.intent())?,
    };
    let state = execute_release(&capability, &store, &permit, &envelope, verified_artifacts.as_ref())?;
    let receipt =
        ReleaseReceiptV1::from_state(capability.intent(), &state).map_err(|error| CliError::ReleaseReceiptIssue {
            detail: error.to_string(),
        })?;
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

/// The directory an explicit `--state` path names. `Path::parent` of a bare
/// filename is `Some("")`, which every later join silently resolves against
/// the process cwd -- so a bare `--state release.json` would put the workspace
/// lock somewhere other than the workspace.
fn state_directory_of(state: &std::path::Path) -> &std::path::Path {
    match state.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => std::path::Path::new("."),
    }
}

fn source_global(global: &GlobalArgs, source_root: Option<&std::path::Path>) -> GlobalArgs {
    let mut source = global.clone();
    if let Some(root) = source_root {
        source.cwd = root.to_path_buf();
    }
    source
}

fn read_intent(path: &std::path::Path) -> Result<ReleaseIntentV1, CliError> {
    let value = read_json_file(path)?;
    // A stale intent is the expected failure after a schema bump; name it
    // before serde reports it as an opaque parse error.
    let found = &value["schemaVersion"];
    if found != &serde_json::json!(ReleaseIntentV1::SCHEMA_VERSION) {
        return Err(CliError::ReleaseIntentSchemaUnsupported {
            path: path.to_path_buf(),
            found: found.to_string(),
            expected: ReleaseIntentV1::SCHEMA_VERSION,
        });
    }
    serde_json::from_value(value).map_err(|error| CliError::ReleaseIntentInvalid {
        path: path.display().to_string(),
        detail: error.to_string(),
    })
}

fn read_artifact_manifest(path: &std::path::Path) -> Result<ArtifactManifestV1, CliError> {
    serde_json::from_value(read_json_file(path)?).map_err(|error| CliError::ArtifactManifestFileInvalid {
        path: path.display().to_string(),
        detail: error.to_string(),
    })
}

fn read_json_file(path: &std::path::Path) -> Result<serde_json::Value, CliError> {
    let content = std::fs::read_to_string(path).map_err(|source| CliError::Io {
        source,
        path: Some(path.to_path_buf()),
    })?;
    serde_json::from_str(&content).map_err(|error| CliError::ReleaseJsonInvalid {
        path: path.display().to_string(),
        detail: error.to_string(),
    })
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

#[cfg(test)]
mod tests {
    use super::state_directory_of;
    use std::path::Path;

    /// `Path::parent` answers `Some("")` for a bare filename. An empty path is
    /// not a directory any join can be reasoned about, so it is normalized to
    /// `.` before it becomes the workspace lock's base directory.
    #[test]
    fn a_bare_state_filename_yields_the_current_directory() {
        assert_eq!(state_directory_of(Path::new("release-state.json")), Path::new("."));
        assert_eq!(state_directory_of(Path::new("state/run.json")), Path::new("state"));
        assert_eq!(state_directory_of(Path::new("/tmp/run.json")), Path::new("/tmp"));
        assert_eq!(state_directory_of(Path::new("/")), Path::new("."));
    }
}
