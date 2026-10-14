use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use callisto_manifests::{open, OpenContext, WorkspaceCargoResolver};
use callisto_model::{ApplyPermit, CommandError, CommandRunner, LockfileRefreshResult, ManifestRole};

use crate::cascade::DepWriteTarget;
use crate::error::GraphError;
use crate::plan::{VersionPlan, VersionWriteTarget};

/// Options governing how a version plan is applied to the workspace.
#[derive(Clone, Debug, Default)]
pub struct ApplyOptions {
    /// When true, regenerate each affected ecosystem's lockfile after applying, in transient mode too.
    pub refresh_lockfiles: bool,
    /// When true (snapshot mode), manifest mutations are written to disk
    /// but changelog prepends (step 7), changeset deletions (step 8), and
    /// git staging (step 11) are suppressed.
    pub transient: bool,
}

/// The result of a successful [`apply_version_plan`] call, describing which paths were written and staged.
#[derive(Clone, Debug, Default)]
pub struct ApplyOutcome {
    /// One entry per lockfile refreshed by this apply; `None` when refresh was skipped or nothing ran.
    pub lockfile_refresh_results: Option<Vec<LockfileRefreshResult>>,
    /// Paths written and staged via `git add`, relative to the workspace root.
    pub staged: Vec<PathBuf>,
}

#[derive(Debug, Default)]
pub(crate) struct ManifestWriteGroup {
    pub(crate) bump: Option<(usize, callisto_model::Version)>,
    pub(crate) rewrite_indices: Vec<usize>,
}

#[derive(Debug, Default)]
pub(crate) struct ManifestWriteClassification {
    pub(crate) batched: BTreeMap<PathBuf, ManifestWriteGroup>,
    /// Root-manifest paths written exclusively through
    /// `WorkspaceCargoResolver` (`CargoWorkspacePackage`/
    /// `CargoWorkspaceDependency` targets), with no competing write to the
    /// same physical file through the `Manifest` trait's `open()` path.
    /// Consumed the same way as `batched`, but via `WorkspaceCargoResolver`
    /// instead of `open()` -- see `apply_version_plan`'s resolver-batched
    /// loop.
    pub(crate) resolver_batched: BTreeMap<PathBuf, ManifestWriteGroup>,
    /// Paths that receive writes through BOTH the `Manifest` trait and
    /// `WorkspaceCargoResolver` -- the mixed-routing data-loss/ordering
    /// hazard.
    /// Never batched on either side; each write is applied individually,
    /// strictly in plan order, exactly as before this change.
    pub(crate) excluded: BTreeSet<PathBuf>,
}

pub(crate) fn classify_manifest_writes(plan: &VersionPlan) -> ManifestWriteClassification {
    let mut resolver_by_path: BTreeMap<PathBuf, ManifestWriteGroup> = BTreeMap::new();
    for (idx, bump) in plan.bumps.iter().enumerate() {
        for write in &bump.writes {
            if let VersionWriteTarget::CargoWorkspacePackage { root_manifest } = write {
                resolver_by_path.entry(root_manifest.clone()).or_default().bump = Some((idx, bump.to.clone()));
            }
        }
    }
    for (idx, rewrite) in plan.rewrites.iter().enumerate() {
        if let DepWriteTarget::CargoWorkspaceDependency { root_manifest } = &rewrite.key.target {
            resolver_by_path
                .entry(root_manifest.clone())
                .or_default()
                .rewrite_indices
                .push(idx);
        }
    }

    let mut by_path: BTreeMap<PathBuf, ManifestWriteGroup> = BTreeMap::new();
    for (idx, bump) in plan.bumps.iter().enumerate() {
        for write in &bump.writes {
            if let VersionWriteTarget::Manifest(p) = write {
                by_path.entry(p.clone()).or_default().bump = Some((idx, bump.to.clone()));
            }
        }
    }
    for (idx, rewrite) in plan.rewrites.iter().enumerate() {
        if let DepWriteTarget::Manifest(p) = &rewrite.key.target {
            by_path.entry(p.clone()).or_default().rewrite_indices.push(idx);
        }
    }

    let mut batched = BTreeMap::new();
    let mut excluded = BTreeSet::new();
    for (path, group) in by_path {
        if resolver_by_path.contains_key(&path) {
            excluded.insert(path);
        } else {
            batched.insert(path, group);
        }
    }

    // Every path in `resolver_by_path` that isn't also mixed-routed
    // (present in `excluded`) is safe to batch into one
    // load/mutate*/persist cycle -- it's written exclusively through
    // WorkspaceCargoResolver, never through the Manifest trait.
    let resolver_batched: BTreeMap<PathBuf, ManifestWriteGroup> = resolver_by_path
        .into_iter()
        .filter(|(path, _)| !excluded.contains(path))
        .collect();

    ManifestWriteClassification {
        batched,
        resolver_batched,
        excluded,
    }
}

/// Writes `plan` to disk and stages the touched paths in git.
///
/// Every side effect here is unconditional -- whether to apply at all is
/// the caller's decision, which is why an [`ApplyPermit`] is required
/// rather than a `dry_run` bool: a bool carries no guarantee any caller
/// actually consulted it. A dry-run caller can't obtain a permit and
/// simply doesn't call this function; it reports `plan` instead.
///
/// # Errors
///
/// - Manifest parse/write failures (malformed TOML/JSON, unsupported format).
/// - Git subprocess failures (`git add`/`git rm --cached` non-zero exit).
/// - I/O errors writing changelog sections or `pre.json`.
pub fn apply_version_plan<R: CommandRunner>(
    root: &Path,
    plan: &VersionPlan,
    runner: &R,
    opts: &ApplyOptions,
    permit: &ApplyPermit,
) -> Result<ApplyOutcome, GraphError> {
    let mut outcome = ApplyOutcome::default();
    let mut modified_paths = Vec::new();
    let mut deferred_deletions = Vec::new();

    let ctx = OpenContext::for_workspace_root(root);

    let classification = classify_manifest_writes(plan);

    for (path, group) in &classification.batched {
        let fmt = callisto_model::ManifestFormat::from_path(path)?;
        let decl = callisto_model::ManifestDecl::new(path.clone(), ManifestRole::Canonical, fmt)?;
        let mut handle = open(&decl, &ctx)?;
        let mut mutated = false;

        if let Some((bump_idx, target_version)) = &group.bump {
            let bump = &plan.bumps[*bump_idx];
            let current = handle.current_version()?;
            if current == *target_version {
                // Already at target — skip write, fall through to rewrites.
            } else if current == bump.from {
                handle.write_version(target_version, permit)?;
                mutated = true;
            } else {
                return Err(GraphError::UnexpectedManifestVersion {
                    path: path.clone(),
                    expected_from: bump.from.clone(),
                    expected_to: bump.to.clone(),
                    found: current,
                });
            }
        }

        for rewrite_idx in &group.rewrite_indices {
            let rewrite = &plan.rewrites[*rewrite_idx];
            handle.update_dependency_spec(
                &rewrite.key.name,
                rewrite.key.kind.unwrap_or(callisto_model::DepKind::Runtime),
                rewrite.to.clone(),
                permit,
            )?;
            mutated = true;
        }

        if mutated {
            handle.persist(permit)?;
        }
        modified_paths.push(path.clone());
    }

    // Root-manifest paths written exclusively through WorkspaceCargoResolver
    // (no competing Manifest-trait write to the same physical file, i.e. not
    // in `classification.excluded`): open once, apply the bump (if any) then
    // every rewrite in memory, and persist exactly once -- mirroring the
    // `classification.batched` loop above, but via WorkspaceCargoResolver
    // instead of `open()`. `CargoWorkspacePackage`/`CargoWorkspaceDependency`
    // writes have never had an idempotency precondition check (unlike
    // `VersionWriteTarget::Manifest`/`DepWriteTarget::Manifest`), so none is
    // introduced here -- both continue to write unconditionally.
    for (root_manifest, group) in &classification.resolver_batched {
        let mut ws_res = WorkspaceCargoResolver::load(&root.join(root_manifest))?;
        let mut mutated = false;

        if let Some((_, target_version)) = &group.bump {
            ws_res.write_version(target_version, permit)?;
            mutated = true;
        }

        for rewrite_idx in &group.rewrite_indices {
            let rewrite = &plan.rewrites[*rewrite_idx];
            ws_res.write_dependency(&rewrite.key.name, rewrite.to.clone(), permit)?;
            mutated = true;
        }

        if mutated {
            ws_res.persist(permit)?;
        }
        modified_paths.push(root_manifest.clone());
    }

    for bump in &plan.bumps {
        for write in &bump.writes {
            match write {
                VersionWriteTarget::Manifest(p) => {
                    if !classification.excluded.contains(p) {
                        continue;
                    }
                    let fmt = callisto_model::ManifestFormat::from_path(p)?;
                    let decl = callisto_model::ManifestDecl::new(p.clone(), ManifestRole::Canonical, fmt)?;
                    let mut handle = open(&decl, &ctx)?;
                    let current = handle.current_version()?;
                    if current == bump.to {
                        // Already at target — skip write but still stage so git add re-stages on retry.
                    } else if current == bump.from {
                        handle.write_version(&bump.to, permit)?;
                        handle.persist(permit)?;
                    } else {
                        return Err(GraphError::UnexpectedManifestVersion {
                            path: p.clone(),
                            expected_from: bump.from.clone(),
                            expected_to: bump.to.clone(),
                            found: current,
                        });
                    }
                    modified_paths.push(p.clone());
                }
                VersionWriteTarget::CargoWorkspacePackage { root_manifest } => {
                    if classification.resolver_batched.contains_key(root_manifest) {
                        // Already applied and persisted by the resolver-batched loop above.
                        continue;
                    }
                    let mut ws_res = WorkspaceCargoResolver::load(&root.join(root_manifest))?;
                    ws_res.write_version(&bump.to, permit)?;
                    ws_res.persist(permit)?;
                    modified_paths.push(root_manifest.clone());
                }
            }
        }
    }

    for rewrite in &plan.rewrites {
        match &rewrite.key.target {
            DepWriteTarget::Manifest(p) => {
                if !classification.excluded.contains(p) {
                    continue;
                }
                let fmt = callisto_model::ManifestFormat::from_path(p)?;
                let decl = callisto_model::ManifestDecl::new(p.clone(), ManifestRole::Canonical, fmt)?;
                let mut handle = open(&decl, &ctx)?;
                handle.update_dependency_spec(
                    &rewrite.key.name,
                    rewrite.key.kind.unwrap_or(callisto_model::DepKind::Runtime),
                    rewrite.to.clone(),
                    permit,
                )?;
                handle.persist(permit)?;
                modified_paths.push(p.clone());
            }
            DepWriteTarget::CargoWorkspaceDependency { root_manifest } => {
                if classification.resolver_batched.contains_key(root_manifest) {
                    // Already applied and persisted by the resolver-batched loop above.
                    continue;
                }
                let mut ws_res = WorkspaceCargoResolver::load(&root.join(root_manifest))?;
                ws_res.write_dependency(&rewrite.key.name, rewrite.to.clone(), permit)?;
                ws_res.persist(permit)?;
                modified_paths.push(root_manifest.clone());
            }
        }
    }

    for pw in &plan.platform_writes {
        let fmt = callisto_model::ManifestFormat::from_path(&pw.manifest)?;
        let decl = callisto_model::ManifestDecl::new(pw.manifest.clone(), ManifestRole::Canonical, fmt)?;
        let mut handle = open(&decl, &ctx)?;
        let current = handle.current_version()?;
        if current == pw.version {
            // Already at target — skip write, but still stage so git add re-stages on retry.
        } else if current == pw.from {
            handle.write_version(&pw.version, permit)?;
            handle.persist(permit)?;
        } else {
            return Err(GraphError::UnexpectedManifestVersion {
                path: pw.manifest.clone(),
                expected_from: pw.from.clone(),
                expected_to: pw.version.clone(),
                found: current,
            });
        }
        modified_paths.push(pw.manifest.clone());
    }

    for opt in &plan.optional_dep_updates {
        let fmt = callisto_model::ManifestFormat::from_path(&opt.manifest)?;
        let decl = callisto_model::ManifestDecl::new(opt.manifest.clone(), ManifestRole::Canonical, fmt)?;
        let mut handle = open(&decl, &ctx)?;
        handle.update_optional_dependencies(&opt.updates, permit)?;
        handle.persist(permit)?;
        modified_paths.push(opt.manifest.clone());
    }

    if !opts.transient {
        for cl in &plan.changelog_writes {
            let rendered = crate::changelog::render_section(&cl.input)?;
            crate::changelog::prepend(
                root,
                &cl.changelog_path,
                &cl.input.package.display_name(),
                &rendered,
                permit,
            )?;
            modified_paths.push(cl.changelog_path.clone());
        }

        for cs_path in &plan.consumed_changesets {
            deferred_deletions.push(cs_path.clone());
        }

        if let Some(ref pre_state) = plan.pre_state_update {
            // Written from `plan.pre_json_path`, not derived from `consumed_changesets` (always empty in pre mode).
            let rel_pre_path = plan.pre_json_path.clone();
            let pre_path = root.join(&rel_pre_path);
            let text = match &plan.pre_json_original_text {
                Some(existing) => callisto_model::format::write_pre_json_preserving(pre_state, existing),
                None => callisto_model::format::write_pre_json(pre_state),
            };
            callisto_manifests::atomic::atomic_write(&pre_path, &text, permit).map_err(|e| GraphError::ApplyIo {
                path: rel_pre_path.clone(),
                message: e.to_string(),
            })?;
            modified_paths.push(rel_pre_path);
        } else if let Some(rel_pre_path) = &plan.delete_pre_json {
            deferred_deletions.push(rel_pre_path.clone());
        }
    }

    // Collect the set of ecosystems actively involved in this plan so that
    // only the corresponding lockfiles are staged. Staging lockfiles whose
    // ecosystem was not touched can sweep up unrelated user changes.
    use callisto_model::Ecosystem;
    let active_ecosystems: std::collections::HashSet<Ecosystem> = plan
        .bumps
        .iter()
        .flat_map(|bump| {
            bump.package
                .ecosystem()
                .into_iter()
                .chain(bump.writes.iter().filter_map(|write| {
                    match write {
                        VersionWriteTarget::CargoWorkspacePackage { .. } => Some(Ecosystem::Cargo),
                        VersionWriteTarget::Manifest(path) => callisto_model::ManifestFormat::from_path(path)
                            .ok()
                            .map(|fmt| fmt.ecosystem()),
                    }
                }))
        })
        .chain(plan.rewrites.iter().filter_map(|rewrite| {
            match &rewrite.key.target {
                DepWriteTarget::CargoWorkspaceDependency { .. } => Some(Ecosystem::Cargo),
                DepWriteTarget::Manifest(path) => callisto_model::ManifestFormat::from_path(path)
                    .ok()
                    .map(|fmt| fmt.ecosystem()),
            }
        }))
        .chain(
            plan.platform_writes
                .iter()
                .filter_map(|write| callisto_model::ManifestFormat::from_path(&write.manifest).ok())
                .map(|fmt| fmt.ecosystem()),
        )
        .chain(
            plan.optional_dep_updates
                .iter()
                .filter_map(|update| callisto_model::ManifestFormat::from_path(&update.manifest).ok())
                .map(|fmt| fmt.ecosystem()),
        )
        .collect();

    // Must run before the git-staging loop below, so refreshed lockfiles are on disk to be staged.
    if opts.refresh_lockfiles {
        let mut refresh_results: Vec<LockfileRefreshResult> = Vec::new();

        if active_ecosystems.contains(&Ecosystem::Cargo) {
            refresh_results.push(run_lockfile_refresh(
                runner,
                root,
                "Cargo.lock",
                "cargo",
                &["update", "--workspace"],
            )?);
        }

        if active_ecosystems.contains(&Ecosystem::Pypi) {
            if root.join("uv.lock").exists() {
                refresh_results.push(run_lockfile_refresh(runner, root, "uv.lock", "uv", &["lock"])?);
            } else if root.join("poetry.lock").exists() {
                refresh_results.push(run_lockfile_refresh(
                    runner,
                    root,
                    "poetry.lock",
                    "poetry",
                    &["lock", "--no-update"],
                )?);
            } else if root.join("pdm.lock").exists() {
                refresh_results.push(run_lockfile_refresh(
                    runner,
                    root,
                    "pdm.lock",
                    "pdm",
                    &["lock", "--update-reuse"],
                )?);
            }
        }

        if active_ecosystems.contains(&Ecosystem::Npm) {
            if root.join("package-lock.json").exists() {
                refresh_results.push(run_lockfile_refresh(
                    runner,
                    root,
                    "package-lock.json",
                    "npm",
                    &["install", "--package-lock-only", "--ignore-scripts"],
                )?);
            }
            if root.join("pnpm-lock.yaml").exists() {
                refresh_results.push(run_lockfile_refresh(
                    runner,
                    root,
                    "pnpm-lock.yaml",
                    "pnpm",
                    &["install", "--lockfile-only"],
                )?);
            }
            if root.join("yarn.lock").exists() {
                refresh_results.push(run_lockfile_refresh(
                    runner,
                    root,
                    "yarn.lock",
                    "yarn",
                    &["install", "--mode", "update-lockfile"],
                )?);
            }
            if root.join("bun.lock").exists() {
                refresh_results.push(run_lockfile_refresh(
                    runner,
                    root,
                    "bun.lock",
                    "bun",
                    &["install", "--lockfile-only"],
                )?);
            } else if root.join("bun.lockb").exists() {
                refresh_results.push(run_lockfile_refresh(
                    runner,
                    root,
                    "bun.lockb",
                    "bun",
                    &["install", "--lockfile-only"],
                )?);
            }
        }

        if !refresh_results.is_empty() {
            outcome.lockfile_refresh_results = Some(refresh_results);
        }
    }

    // Map each well-known lockfile to its ecosystem, then include the file
    // only when that ecosystem is active and the file exists on disk.
    let lockfile_ecosystems: &[(&str, Ecosystem)] = &[
        ("Cargo.lock", Ecosystem::Cargo),
        ("package-lock.json", Ecosystem::Npm),
        ("pnpm-lock.yaml", Ecosystem::Npm),
        ("yarn.lock", Ecosystem::Npm),
        ("bun.lock", Ecosystem::Npm),
        ("bun.lockb", Ecosystem::Npm),
        ("uv.lock", Ecosystem::Pypi),
        ("poetry.lock", Ecosystem::Pypi),
        ("pdm.lock", Ecosystem::Pypi),
        ("Pipfile.lock", Ecosystem::Pypi),
    ];
    for (lockfile, ecosystem) in lockfile_ecosystems {
        if !active_ecosystems.contains(ecosystem) {
            continue;
        }
        let p = PathBuf::from(lockfile);
        if root.join(&p).exists() && !modified_paths.contains(&p) {
            modified_paths.push(p);
        }
    }

    if !opts.transient && (!modified_paths.is_empty() || !deferred_deletions.is_empty()) {
        // Stage all written release state before consuming its inputs. If a refresh or add fails,
        // the changesets/pre-state remain available; if removal staging fails, the manifest diff
        // still identifies the interrupted run.
        let (existing, deleted): (Vec<_>, Vec<_>) = modified_paths.iter().partition(|p| root.join(p).exists());

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
                    stderr: output.redacted_stderr(),
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
                    stderr: output.redacted_stderr(),
                }));
            }
        }

        if !deferred_deletions.is_empty() {
            let mut args = vec!["rm", "--cached", "--ignore-unmatch", "--"];
            let strs: Vec<String> = deferred_deletions.iter().map(|p| p.display().to_string()).collect();
            for s in &strs {
                args.push(s);
            }
            let output = runner.run("git", &args, root)?;
            if !output.success() {
                return Err(GraphError::Command(CommandError::Failed {
                    program: "git".to_string(),
                    exit_code: output.exit_code,
                    stderr: output.redacted_stderr(),
                }));
            }

            // Keep the inputs on disk until their removal is staged so a failed index update can be retried.
            for path in &deferred_deletions {
                let full = root.join(path);
                if full.exists() {
                    fs::remove_file(&full).map_err(|e| GraphError::ApplyIo {
                        path: path.clone(),
                        message: e.to_string(),
                    })?;
                }
            }
            modified_paths.extend(deferred_deletions.iter().cloned());
        }

        outcome.staged = modified_paths;
    }

    Ok(outcome)
}

/// Maps a non-zero exit to `GraphError::LockfileRefreshFailed`, an exec failure to `GraphError::Command`.
fn run_lockfile_refresh<R: CommandRunner>(
    runner: &R,
    root: &Path,
    filename: &str,
    program: &str,
    args: &[&str],
) -> Result<LockfileRefreshResult, GraphError> {
    let refresh_command = format!("{program} {}", args.join(" "));
    let out = runner.run(program, args, root)?;
    if !out.success() {
        return Err(GraphError::LockfileRefreshFailed {
            filename: PathBuf::from(filename),
            refresh_command,
            exit_code: out.exit_code,
            stderr: out.redacted_stderr(),
        });
    }
    Ok(LockfileRefreshResult {
        filename: PathBuf::from(filename),
        refresh_command,
        success: true,
        exit_code: out.exit_code,
    })
}

#[cfg(test)]
mod tests;
