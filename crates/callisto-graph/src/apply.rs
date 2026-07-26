use std::fs;
use std::path::{Path, PathBuf};

use callisto_manifests::{open, OpenContext, WorkspaceCargoResolver};
use callisto_model::{CommandRunner, LockfileRefreshResult, ManifestRole};

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
                        let decl = callisto_model::ManifestDecl::new(
                            p.clone(),
                            ManifestRole::Canonical,
                            if p.ends_with("Cargo.toml") {
                                callisto_model::ManifestFormat::CargoToml
                            } else {
                                callisto_model::ManifestFormat::PackageJson
                            },
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
                    let decl = callisto_model::ManifestDecl::new(
                        p.clone(),
                        ManifestRole::Canonical,
                        if p.ends_with("Cargo.toml") {
                            callisto_model::ManifestFormat::CargoToml
                        } else {
                            callisto_model::ManifestFormat::PackageJson
                        },
                    )?;
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
                let _ = fs::remove_file(&full);
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
            let pre_path = root.join(pre_dir.join("pre.json"));
            let text = callisto_format::write_pre_json(pre_state);
            let _ = callisto_manifests::atomic::atomic_write(&pre_path, &text);
        } else if plan.delete_pre_json {
            let pre_path = root.join(".changeset/pre.json");
            if pre_path.exists() {
                let _ = fs::remove_file(&pre_path);
            }
        }
    }

    if !opts.transient && !modified_paths.is_empty() {
        let strings: Vec<String> = modified_paths
            .iter()
            .filter(|p| root.join(p).exists())
            .map(|p| p.display().to_string())
            .collect();
        if !strings.is_empty() {
            let mut path_strs = vec!["add", "--"];
            for s in &strings {
                path_strs.push(s);
            }
            let output = runner.run("git", &path_strs, root)?;
            if output.success() {
                outcome.staged = modified_paths;
            }
        }
    }

    Ok(outcome)
}
