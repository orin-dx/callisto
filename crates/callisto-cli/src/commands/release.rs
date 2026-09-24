//! Release interfaces: bare `release` (local), and the CI plan/execute route.
//!
//! Bare `release` publishes every unreleased package through the same
//! `execute_release` route as `release execute`. The CI route accepts only
//! exact package identities and explicit intent, receipt, and orchestration
//! revision paths.

use std::process::ExitCode;

use callisto_graph::commands::{
    build_release_intent, build_release_intent_with_artifacts, ci_release_route, derive_release_commit_decision,
    derive_selected_release_decision, execute_release, plan_local_release, validate_local_release_intent,
    validate_release_intent, verify_artifact_manifest, LocalReleaseSource, VersionOptions,
};
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_model::{
    ApplyPermit, ArtifactDigest, ArtifactManifestEntryV1, ArtifactManifestV1, ExecutionTrustProfileV1,
    GitHubArtifactAttestationV1, ReleaseIntentV1, ReleasePackageId, ReleaseReceiptV1, ReleaseRunEnvelopeV1,
};

use crate::cli::{
    GlobalArgs, OutputFormat, ReleaseArgs, ReleaseArtifactManifestArgs, ReleaseCommandArgs, ReleaseExecuteArgs,
    ReleaseInspectArgs, ReleasePlanArgs,
};
use crate::error::CliError;
use crate::output::{log_line, write_json};
use crate::runner::CliCommandRunner;
use crate::workspace::{load_workspace, select_inference};

pub fn handle(args: ReleaseCommandArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    match args.command {
        None => release(&args.packages, args.receipt, global),
        Some(ReleaseArgs::Plan(args)) => plan(args, global),
        Some(ReleaseArgs::Inspect(args)) => inspect(args, global),
        Some(ReleaseArgs::ArtifactManifest(args)) => artifact_manifest(args, global),
        Some(ReleaseArgs::Execute(args)) => execute(args, global),
    }
}

/// Stderr note when a preview's tags have no `origin` to push to.
pub const TAGS_UNBOUND_NOTE: &str =
    "note: `origin` has no push URL, so tag operations are unbound; `callisto release` will refuse until one is set";

/// Bound on the `gh auth status` credential probe.
const GH_AUTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Exact stdout when no package has an unreleased version.
pub const NOTHING_TO_RELEASE: &str = "Nothing to release.";

fn release(
    packages: &[String],
    receipt: Option<std::path::PathBuf>,
    global: &GlobalArgs,
) -> Result<ExitCode, CliError> {
    let selections = parse_selections(packages)?;
    let runner = CliCommandRunner;
    let workspace = load_workspace(global, &runner)?;
    let root = workspace.root.clone();
    let locator = IgnoreWalkLocator::new(&root);
    if global.dry_run {
        match plan_local_release(&root, &locator, &runner, &selections, LocalReleaseSource::Preview)? {
            None => print_nothing_to_release(global)?,
            Some(plan) => {
                match global.format {
                    OutputFormat::Json => write_json(&mut std::io::stdout(), &plan.intent)?,
                    OutputFormat::Text => print!("{}", render_release_plan(&plan.intent)),
                }
                if plan.tags_unbound {
                    eprintln!("{TAGS_UNBOUND_NOTE}");
                }
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(route) = ci_release_route(&workspace)? {
        return Err(CliError::ReleaseRequiresCiRoute {
            reason: route.to_string(),
        });
    }
    let Some(plan) = plan_local_release(&root, &locator, &runner, &selections, LocalReleaseSource::Trusted)? else {
        print_nothing_to_release(global)?;
        return Ok(ExitCode::SUCCESS);
    };
    let intent = plan.intent;
    let var = |name: &str| std::env::var(name).ok();
    // Quiet and bounded: `gh auth status` prints account details this command must not echo.
    let gh_authenticated = || {
        callisto_model::CommandRunner::run_quiet(&runner, "gh", &["auth", "status"], &root, GH_AUTH_TIMEOUT)
            .is_ok_and(|output| output.success())
    };
    super::release_credentials::check(
        &intent,
        &super::release_credentials::CredentialSources {
            var: &var,
            home: home_dir(),
            root: &root,
            package_dirs: &plan.package_dirs,
            gh_authenticated: &gh_authenticated,
        },
    )?;
    let receipt = match receipt {
        Some(path) => explicit_receipt_path(&root, &path)?,
        None => default_receipt_path(&root, &intent, &var)?,
    };
    let permit = ApplyPermit::granted_unless_dry_run(false).expect("non-dry-run permits writes");
    // A receipt records only full success; a partial run is recovered by rerunning.
    callisto_model::atomic::probe_atomic_write(&receipt, &permit).map_err(|source| CliError::Io {
        source,
        path: Some(receipt.clone()),
    })?;
    let callisto_model::SourceIdentity::GitCommit { sha } = &intent.snapshot.source else {
        return Err(CliError::ReleaseEnvelopeInvalid {
            detail: "a local release source must be a Git commit".to_owned(),
        });
    };
    // A local run is its own orchestration, so both revisions are the source commit.
    let envelope =
        ReleaseRunEnvelopeV1::new(sha.clone(), &intent, None).map_err(|error| CliError::ReleaseEnvelopeInvalid {
            detail: error.to_string(),
        })?;
    let capability = validate_local_release_intent(&root, &locator, &runner, intent)?;
    let state = execute_release(&capability, &permit, &envelope, None)?;
    let receipt_document =
        ReleaseReceiptV1::from_state(capability.intent(), &state).map_err(|error| CliError::ReleaseReceiptIssue {
            detail: error.to_string(),
        })?;
    write_receipt(&receipt, &receipt_document, &permit)?;
    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &receipt_document)?,
        OutputFormat::Text => println!(
            "Released {} package(s); receipt saved to {}",
            capability.intent().decision.entries.len(),
            receipt.display()
        ),
    }
    Ok(ExitCode::SUCCESS)
}

fn print_nothing_to_release(global: &GlobalArgs) -> Result<(), CliError> {
    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &serde_json::json!({ "nothingToRelease": true }))?,
        OutputFormat::Text => println!("{NOTHING_TO_RELEASE}"),
    }
    Ok(())
}

fn parse_selections(packages: &[String]) -> Result<Vec<ReleasePackageId>, CliError> {
    packages
        .iter()
        .map(|raw| {
            ReleasePackageId::parse(raw).map_err(|error| CliError::ReleasePackageInvalid {
                raw: raw.clone(),
                detail: error.to_string(),
            })
        })
        .collect()
}

/// Human-readable plan: each package and version, then its operations.
fn render_release_plan(intent: &ReleaseIntentV1) -> String {
    let mut out = String::from("Release plan:\n");
    for entry in &intent.decision.entries {
        out.push_str(&format!("  {} {}\n", entry.package, entry.target_version));
        for operation in intent.operations.iter().filter(|op| op.id().package == entry.package) {
            let step = match &operation.id().role {
                callisto_model::ReleaseOperationRole::RegistryPublish { registry } => {
                    format!("publish to {}", registry.registry_key().as_str())
                }
                callisto_model::ReleaseOperationRole::PlatformPublish { registry, platform } => {
                    format!("publish {} to {}", platform.name(), registry.registry_key().as_str())
                }
                callisto_model::ReleaseOperationRole::Tag => "create git tag".to_owned(),
                callisto_model::ReleaseOperationRole::ForgeRelease => "create GitHub release draft".to_owned(),
                callisto_model::ReleaseOperationRole::ArtifactUpload { slot } => {
                    format!("upload {}", slot.asset_name)
                }
                callisto_model::ReleaseOperationRole::ForgePublish => "publish GitHub release".to_owned(),
            };
            out.push_str(&format!("    - {step}\n"));
        }
    }
    out
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(std::path::PathBuf::from)
}

/// `<platform state dir>/callisto/<repo-hash>/<intent-digest>/receipt.json`, never inside `root`.
fn default_receipt_path(
    root: &std::path::Path,
    intent: &ReleaseIntentV1,
    var: &dyn Fn(&str) -> Option<String>,
) -> Result<std::path::PathBuf, CliError> {
    let state = state_dir(var, home_dir()).ok_or_else(|| CliError::ReleaseReceiptLocation {
        detail: "no platform state directory (set XDG_STATE_HOME or HOME)".to_owned(),
    })?;
    let root = canonical_prefix(root)?;
    let path = receipt_path_in(&canonical_prefix(&state)?, &root, intent);
    if path.starts_with(&root) {
        return Err(CliError::ReleaseReceiptLocation {
            detail: format!("the state directory `{}` is inside the repository", state.display()),
        });
    }
    Ok(path)
}

/// A `--receipt` path, refused inside the repository.
fn explicit_receipt_path(root: &std::path::Path, path: &std::path::Path) -> Result<std::path::PathBuf, CliError> {
    let absolute = std::path::absolute(path).map_err(|source| CliError::Io {
        source,
        path: Some(path.to_path_buf()),
    })?;
    if canonical_prefix(&absolute)?.starts_with(canonical_prefix(root)?) {
        return Err(CliError::ReleaseReceiptLocation {
            detail: format!("`{}` is inside the repository", path.display()),
        });
    }
    Ok(absolute)
}

/// Canonicalizes the longest existing ancestor of `path` and re-appends the rest.
fn canonical_prefix(path: &std::path::Path) -> Result<std::path::PathBuf, CliError> {
    let mut existing = path;
    let mut rest = Vec::new();
    while !existing.exists() {
        let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) else {
            return Ok(path.to_path_buf());
        };
        rest.push(name.to_owned());
        existing = parent;
    }
    let mut canonical = dunce::canonicalize(existing).map_err(|source| CliError::Io {
        source,
        path: Some(existing.to_path_buf()),
    })?;
    canonical.extend(rest.into_iter().rev());
    Ok(canonical)
}

fn receipt_path_in(state: &std::path::Path, root: &std::path::Path, intent: &ReleaseIntentV1) -> std::path::PathBuf {
    use sha2::Digest as _;
    let repo_hash = format!("{:x}", sha2::Sha256::digest(root.to_string_lossy().as_bytes()));
    state
        .join("callisto")
        .join(&repo_hash[..16])
        .join(intent.digest().to_string())
        .join("receipt.json")
}

/// `$XDG_STATE_HOME`, else the platform default for per-user state.
fn state_dir(var: &dyn Fn(&str) -> Option<String>, home: Option<std::path::PathBuf>) -> Option<std::path::PathBuf> {
    if let Some(xdg) = var("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Some(xdg);
    }
    if cfg!(windows) {
        if let Some(local) = var("LOCALAPPDATA").filter(|value| !value.is_empty()) {
            return Some(std::path::PathBuf::from(local));
        }
    }
    let home = home?;
    Some(if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else {
        home.join(".local").join("state")
    })
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
            let selections = parse_selections(&args.packages)?;
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
            // A missing forge-repository is the graph's error; only the match lives here.
            if let Some(configured) = workspace
                .config
                .product_release
                .as_ref()
                .and_then(|release| release.forge_repository.as_ref())
                .filter(|configured| **configured != repository)
            {
                return Err(CliError::ReleaseForgeRepositoryMismatch {
                    configured: configured.as_slug(),
                    requested: repository.as_slug(),
                });
            }
            build_release_intent_with_artifacts(
                &workspace.root,
                &locator,
                &runner,
                &decision,
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

fn execute(args: ReleaseExecuteArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    // Inputs and the receipt destination are checked before the first effect; a provider failure
    // mid-run still leaves landed effects without a receipt, and a rerun adopts them.
    let permit = ApplyPermit::granted_unless_dry_run(global.dry_run).ok_or(CliError::ReleaseExecuteDryRun)?;
    let intent = read_intent(&args.intent)?;
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
    // A missing forge-repository is the graph's error during intent validation; only the match lives here.
    if let Some(configured) = source_workspace
        .config
        .product_release
        .as_ref()
        .and_then(|release| release.forge_repository.as_ref())
    {
        if let Some(slot) = intent
            .artifact_slots
            .iter()
            .find(|slot| slot.attestation_policy.repository != *configured)
        {
            return Err(CliError::ReleaseForgeRepositoryMismatch {
                configured: configured.as_slug(),
                requested: slot.attestation_policy.repository.as_slug(),
            });
        }
    }
    // The workspace root, resolved exactly as `plan` resolves it. Executing
    // from a subdirectory must reach the same root, not the process cwd.
    let root = source_workspace.root.clone();
    let locator = IgnoreWalkLocator::new(&root);
    let capability = validate_release_intent(&root, &locator, &runner, intent)?;
    let state = execute_release(&capability, &permit, &envelope, verified_artifacts.as_ref())?;
    let receipt =
        ReleaseReceiptV1::from_state(capability.intent(), &state).map_err(|error| CliError::ReleaseReceiptIssue {
            detail: error.to_string(),
        })?;
    write_receipt(&args.receipt, &receipt, &permit)?;
    match global.format {
        OutputFormat::Json => write_json(&mut std::io::stdout(), &receipt)?,
        OutputFormat::Text => println!("Release receipt saved to {}", args.receipt.display()),
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
    use std::path::{Path, PathBuf};

    use super::*;

    fn sample() -> ReleaseIntentV1 {
        super::super::release_credentials::tests::intent(callisto_model::Ecosystem::Cargo, "cratesIo", true)
    }

    fn vars(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn state_dir_prefers_absolute_xdg_state_home() {
        let home = Some(PathBuf::from("/home/u"));
        assert_eq!(
            state_dir(&vars(&[("XDG_STATE_HOME", "/state")]), home.clone()),
            Some(PathBuf::from("/state"))
        );
        let fallback = state_dir(&vars(&[("XDG_STATE_HOME", "relative")]), home.clone()).unwrap();
        assert!(fallback.starts_with("/home/u"), "{}", fallback.display());
        assert_eq!(state_dir(&vars(&[]), None), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_state_dir_defaults_to_local_state() {
        assert_eq!(
            state_dir(&vars(&[]), Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.local/state"))
        );
    }

    #[test]
    fn receipt_path_is_keyed_by_repository_and_intent() {
        let intent = sample();
        let path = receipt_path_in(Path::new("/state"), Path::new("/repo/a"), &intent);
        let parts: Vec<_> = path.iter().map(|part| part.to_string_lossy().into_owned()).collect();
        assert_eq!(parts[..3], ["/", "state", "callisto"]);
        assert_eq!(parts[3].len(), 16);
        assert_eq!(parts[4], intent.digest().to_string());
        assert_eq!(parts[5], "receipt.json");
        assert_ne!(
            receipt_path_in(Path::new("/state"), Path::new("/repo/b"), &intent),
            path,
            "each repository gets its own directory"
        );
    }

    #[test]
    fn default_receipt_path_refuses_a_state_dir_inside_the_worktree() {
        let intent = sample();
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("state").to_string_lossy().into_owned();
        let var = move |name: &str| (name == "XDG_STATE_HOME").then(|| inside.clone());
        assert!(matches!(
            default_receipt_path(root.path(), &intent, &var),
            Err(CliError::ReleaseReceiptLocation { .. })
        ));
        let outside = tempfile::tempdir().unwrap();
        let outside_path = outside.path().to_string_lossy().into_owned();
        let var = move |name: &str| (name == "XDG_STATE_HOME").then(|| outside_path.clone());
        let path = default_receipt_path(root.path(), &intent, &var).unwrap();
        let canonical_outside = dunce::canonicalize(outside.path()).unwrap();
        let canonical_root = dunce::canonicalize(root.path()).unwrap();
        assert!(path.starts_with(&canonical_outside) && !path.starts_with(&canonical_root));
    }

    /// L2: the repository hash is taken from the canonical root, so a symlinked path agrees.
    #[cfg(unix)]
    #[test]
    fn default_receipt_path_is_stable_across_symlinked_roots() {
        let intent = sample();
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        let state = tempfile::tempdir().unwrap();
        let state_path = state.path().to_string_lossy().into_owned();
        let var = move |name: &str| (name == "XDG_STATE_HOME").then(|| state_path.clone());
        assert_eq!(
            default_receipt_path(&root, &intent, &var).unwrap(),
            default_receipt_path(&link, &intent, &var).unwrap()
        );
    }

    /// L2: `--receipt` inside the repository is refused, including through a symlink.
    #[cfg(unix)]
    #[test]
    fn explicit_receipt_inside_the_worktree_is_refused() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        for inside in [root.join("receipt.json"), link.join("out").join("receipt.json")] {
            assert!(
                matches!(
                    explicit_receipt_path(&root, &inside),
                    Err(CliError::ReleaseReceiptLocation { .. })
                ),
                "{}",
                inside.display()
            );
        }
        let outside = base.path().join("receipts").join("receipt.json");
        assert_eq!(explicit_receipt_path(&root, &outside).unwrap(), outside);
    }

    #[test]
    fn plan_text_lists_each_package_version_and_operation() {
        let intent = sample();
        let text = render_release_plan(&intent);
        assert!(text.starts_with("Release plan:\n"), "{text}");
        for entry in &intent.decision.entries {
            assert!(
                text.contains(&format!("  {} {}\n", entry.package, entry.target_version)),
                "{text}"
            );
        }
        assert_eq!(text.matches("    - ").count(), intent.operations.len(), "{text}");
    }
}
