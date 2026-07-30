use std::fs;
use std::path::{Path, PathBuf};

use callisto_manifests::{open, OpenContext, WorkspaceCargoResolver};
use callisto_model::{CommandError, CommandRunner, LockfileRefreshResult, ManifestRole};

use crate::cascade::DepWriteTarget;
use crate::error::GraphError;
use crate::plan::{VersionPlan, VersionWriteTarget};

#[derive(Clone, Debug, Default)]
pub struct ApplyOptions {
    pub refresh_lockfiles: bool,
    pub transient: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ApplyOutcome {
    pub lockfile_refresh_results: Option<Vec<LockfileRefreshResult>>,
    pub staged: Vec<PathBuf>,
}

pub fn apply_version_plan<R: CommandRunner>(
    root: &Path,
    plan: &VersionPlan,
    runner: &R,
    opts: &ApplyOptions,
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

    if !opts.transient {
        for bump in &plan.bumps {
            for write in &bump.writes {
                match write {
                    VersionWriteTarget::Manifest(p) => {
                        let fmt = callisto_model::ManifestFormat::from_path(p)?;
                        let decl = callisto_model::ManifestDecl::new(
                            p.clone(),
                            ManifestRole::Canonical,
                            fmt,
                        )?;
                        let mut handle = open(&decl, &ctx)?;
                        handle.write_version(&bump.to)?;
                        modified_paths.push(p.clone());
                    }
                    VersionWriteTarget::CargoWorkspacePackage { root_manifest } => {
                        let mut ws_res = WorkspaceCargoResolver::load(&root.join(root_manifest))?;
                        ws_res.write_version(&bump.to)?;
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
                    )?;
                    modified_paths.push(p.clone());
                }
                DepWriteTarget::CargoWorkspaceDependency { root_manifest } => {
                    let mut ws_res = WorkspaceCargoResolver::load(&root.join(root_manifest))?;
                    ws_res.write_dependency(&rewrite.key.name, rewrite.to.clone())?;
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
            callisto_manifests::atomic::atomic_write(&pre_path, &text).map_err(|e| {
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
        ] {
            let p = PathBuf::from(lockfile);
            if root.join(&p).exists() && !modified_paths.contains(&p) {
                modified_paths.push(p);
            }
        }
    }

    if !opts.transient && !modified_paths.is_empty() {
        let (existing, deleted): (Vec<_>, Vec<_>) =
            modified_paths.iter().partition(|p| root.join(p).exists());

        if !existing.is_empty() {
            let mut args = vec!["add", "--"];
            let strs: Vec<String> = existing.iter().map(|p| p.display().to_string()).collect();
            for s in &strs {
                args.push(s);
            }
            drop(runner.run("git", &args, root));
        }

        if !deleted.is_empty() {
            let mut args = vec!["rm", "--cached", "--ignore-unmatch", "--"];
            let strs: Vec<String> = deleted.iter().map(|p| p.display().to_string()).collect();
            for s in &strs {
                args.push(s);
            }
            drop(runner.run("git", &args, root));
        }

        outcome.staged = modified_paths;
    }

    Ok(outcome)
}
