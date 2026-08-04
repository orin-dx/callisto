use std::fs;
use std::path::{Path, PathBuf};

use callisto_manifests::{open, OpenContext, WorkspaceCargoResolver};
use callisto_model::{
    ApplyPermit, CommandError, CommandRunner, LockfileRefreshResult, ManifestRole,
};

use crate::cascade::DepWriteTarget;
use crate::error::GraphError;
use crate::plan::{VersionPlan, VersionWriteTarget};

/// Options governing how a version plan is applied to the workspace.
#[derive(Clone, Debug, Default)]
pub struct ApplyOptions {
    /// Plumbed from `--refresh-lockfiles` but not yet consulted here;
    /// `ApplyOutcome::lockfile_refresh_results` is consequently always `None`.
    pub refresh_lockfiles: bool,
}

/// The result of a successful [`apply_version_plan`] call, describing which paths were written and staged.
#[derive(Clone, Debug, Default)]
pub struct ApplyOutcome {
    /// Reserved for lockfile refresh results; currently always `None`.
    pub lockfile_refresh_results: Option<Vec<LockfileRefreshResult>>,
    /// Paths written and staged via `git add`, relative to the workspace root.
    pub staged: Vec<PathBuf>,
}

/// Writes `plan` to disk and stages the touched paths in git.
///
/// Every side effect this performs is unconditional -- the decision of whether
/// to apply at all belongs to the caller, which is why an [`ApplyPermit`] is
/// required rather than a `dry_run` flag being passed in. The previous
/// `ApplyOptions::transient` field was that flag, and being a plain bool it
/// carried no guarantee that any caller consulted it before constructing the
/// options. A dry-run caller now cannot obtain a permit and simply does not
/// call this function; it reports `plan` instead.
///
/// # Errors
///
/// - Manifest parse or write failures (malformed TOML/JSON, unsupported format).
/// - Git subprocess failures (`git add` or `git rm --cached` returns a non-zero exit code).
/// - I/O errors writing changelog sections or `pre.json`.
pub fn apply_version_plan<R: CommandRunner>(
    root: &Path,
    plan: &VersionPlan,
    runner: &R,
    _opts: &ApplyOptions,
    permit: &ApplyPermit,
) -> Result<ApplyOutcome, GraphError> {
    let mut outcome = ApplyOutcome::default();
    let mut modified_paths = Vec::new();

    let cargo_workspace = if root.join("Cargo.toml").exists() {
        if let Ok(resolver) = WorkspaceCargoResolver::load(&root.join("Cargo.toml")) {
            resolver.inheritance().ok().map(std::sync::Arc::new)
        } else {
            None
        }
    } else {
        None
    };

    let npm_workspace_kind = callisto_manifests::detect_npm_workspace_kind(root)
        .ok()
        .flatten();

    let ctx = OpenContext {
        workspace_root: root,
        cargo_workspace,
        npm_workspace_kind,
    };

    for bump in &plan.bumps {
        for write in &bump.writes {
            match write {
                VersionWriteTarget::Manifest(p) => {
                    let fmt = callisto_model::ManifestFormat::from_path(p)?;
                    let decl =
                        callisto_model::ManifestDecl::new(p.clone(), ManifestRole::Canonical, fmt)?;
                    let mut handle = open(&decl, &ctx)?;
                    handle.write_version(&bump.to, permit)?;
                    modified_paths.push(p.clone());
                }
                VersionWriteTarget::CargoWorkspacePackage { root_manifest } => {
                    let mut ws_res = WorkspaceCargoResolver::load(&root.join(root_manifest))?;
                    ws_res.write_version(&bump.to, permit)?;
                    modified_paths.push(root_manifest.clone());
                }
            }
        }
    }

    for rewrite in &plan.rewrites {
        match &rewrite.key.target {
            DepWriteTarget::Manifest(p) => {
                let fmt = callisto_model::ManifestFormat::from_path(p)?;
                let decl =
                    callisto_model::ManifestDecl::new(p.clone(), ManifestRole::Canonical, fmt)?;
                let mut handle = open(&decl, &ctx)?;
                handle.update_dependency_spec(
                    &rewrite.key.name,
                    rewrite.key.kind.unwrap_or(callisto_model::DepKind::Runtime),
                    rewrite.to.clone(),
                    permit,
                )?;
                modified_paths.push(p.clone());
            }
            DepWriteTarget::CargoWorkspaceDependency { root_manifest } => {
                let mut ws_res = WorkspaceCargoResolver::load(&root.join(root_manifest))?;
                ws_res.write_dependency(&rewrite.key.name, rewrite.to.clone(), permit)?;
                modified_paths.push(root_manifest.clone());
            }
        }
    }

    for cl in &plan.changelog_writes {
        let rendered = callisto_changelog::render_section(&cl.input)?;
        callisto_changelog::prepend(
            root,
            &cl.changelog_path,
            &cl.input.package.display_name(),
            &rendered,
            permit,
        )?;
        modified_paths.push(cl.changelog_path.clone());
    }

    for cs_path in &plan.consumed_changesets {
        let full = root.join(cs_path);
        if full.exists() {
            fs::remove_file(&full).map_err(|e| {
                GraphError::Command(CommandError::Io {
                    program: "fs".to_string(),
                    message: e.to_string(),
                })
            })?;
            modified_paths.push(cs_path.clone());
        }
    }

    if let Some(ref pre_state) = plan.pre_state_update {
        let default_dir = PathBuf::from(".changeset");
        let pre_dir = plan
            .consumed_changesets
            .first()
            .and_then(|p| p.parent())
            .unwrap_or(&default_dir);
        let rel_pre_path = pre_dir.join("pre.json");
        let pre_path = root.join(&rel_pre_path);
        let text = callisto_format::write_pre_json(pre_state);
        callisto_manifests::atomic::atomic_write(&pre_path, &text, permit).map_err(|e| {
            GraphError::Command(CommandError::Io {
                program: "fs".to_string(),
                message: e.to_string(),
            })
        })?;
        modified_paths.push(rel_pre_path);
    } else if plan.delete_pre_json {
        let rel_pre_path = PathBuf::from(".changeset/pre.json");
        let pre_path = root.join(&rel_pre_path);
        if pre_path.exists() {
            fs::remove_file(&pre_path).map_err(|e| {
                GraphError::Command(CommandError::Io {
                    program: "fs".to_string(),
                    message: e.to_string(),
                })
            })?;
            modified_paths.push(rel_pre_path);
        }
    }

    // Include lockfiles if present in workspace root
    for lockfile in &[
        "Cargo.lock",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lockb",
        "uv.lock",
        "poetry.lock",
        "pdm.lock",
        "Pipfile.lock",
    ] {
        let p = PathBuf::from(lockfile);
        if root.join(&p).exists() && !modified_paths.contains(&p) {
            modified_paths.push(p);
        }
    }

    if !modified_paths.is_empty() {
        let (existing, deleted): (Vec<_>, Vec<_>) =
            modified_paths.iter().partition(|p| root.join(p).exists());

        if !existing.is_empty() {
            let mut args = vec!["add", "--"];
            let strs: Vec<String> = existing.iter().map(|p| p.display().to_string()).collect();
            for s in &strs {
                args.push(s);
            }
            let output = runner.run("git", &args, root)?;
            if !output.success() {
                return Err(GraphError::Command(CommandError::Failed {
                    program: "git".to_string(),
                    exit_code: output.exit_code,
                    stderr: output.stderr,
                }));
            }
        }

        if !deleted.is_empty() {
            let mut args = vec!["rm", "--cached", "--ignore-unmatch", "--"];
            let strs: Vec<String> = deleted.iter().map(|p| p.display().to_string()).collect();
            for s in &strs {
                args.push(s);
            }
            let output = runner.run("git", &args, root)?;
            if !output.success() {
                return Err(GraphError::Command(CommandError::Failed {
                    program: "git".to_string(),
                    exit_code: output.exit_code,
                    stderr: output.stderr,
                }));
            }
        }

        outcome.staged = modified_paths;
    }

    Ok(outcome)
}
