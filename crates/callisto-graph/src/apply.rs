use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use callisto_manifests::{open, Manifest, OpenContext, WorkspaceCargoResolver};
use callisto_model::{
    ApplyPermit, CommandError, CommandRunner, DepKind, Ecosystem, LockfileRefreshResult, ManifestFormat, ManifestRole,
    Version,
};

use crate::cascade::DepWriteTarget;
use crate::cascade::SpecRewrite;
use crate::error::GraphError;
use crate::plan::{OptionalDepUpdate, PlannedBump, PlatformWrite, VersionPlan, VersionWriteTarget};

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

/// Whether `root/path` exists; a failure to look is an error, never "deleted".
fn path_exists(root: &Path, path: &Path) -> Result<bool, GraphError> {
    callisto_model::strict_fs::exists(&root.join(path)).map_err(|e| GraphError::ApplyIo {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
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

    for step in write_steps(plan, &classification) {
        modified_paths.push(apply_write_step(&step, plan, root, &ctx, permit)?);
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

    let active_ecosystems = active_ecosystems(plan);

    // Must run before the git-staging loop below, so refreshed lockfiles are on disk to be staged.
    if opts.refresh_lockfiles {
        let refresh_results = refresh_lockfiles(runner, root, &active_ecosystems)?;
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
        if path_exists(root, &p)? && !modified_paths.contains(&p) {
            modified_paths.push(p);
        }
    }

    if !opts.transient && (!modified_paths.is_empty() || !deferred_deletions.is_empty()) {
        // Stage all written release state before consuming its inputs. If a refresh or add fails,
        // the changesets/pre-state remain available; if removal staging fails, the manifest diff
        // still identifies the interrupted run.
        let mut existing = Vec::new();
        let mut deleted = Vec::new();
        for path in &modified_paths {
            if path_exists(root, path)? {
                existing.push(path);
            } else {
                deleted.push(path);
            }
        }

        if !existing.is_empty() {
            run_git_on_paths(runner, root, &["add", "--"], &existing)?;
        }

        if !deleted.is_empty() {
            run_git_on_paths(runner, root, &["rm", "--cached", "--ignore-unmatch", "--"], &deleted)?;
        }

        if !deferred_deletions.is_empty() {
            let deferred: Vec<&PathBuf> = deferred_deletions.iter().collect();
            run_git_on_paths(runner, root, &["rm", "--cached", "--ignore-unmatch", "--"], &deferred)?;

            // Keep the inputs on disk until their removal is staged so a failed index update can be retried.
            for path in &deferred_deletions {
                let full = root.join(path);
                if path_exists(root, path)? {
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

/// One physical write of a version plan, in the order it must happen.
enum WriteStep<'p> {
    /// One load/mutate/persist cycle over a manifest only the `Manifest` trait writes: the
    /// version bump (idempotency-guarded) first, then every dependency rewrite, one persist.
    Manifest {
        path: &'p PathBuf,
        group: &'p ManifestWriteGroup,
    },
    /// The same cycle for a Cargo workspace root only the `WorkspaceCargoResolver` writes. Its
    /// writes carry no idempotency precondition.
    WorkspaceRoot {
        root_manifest: &'p PathBuf,
        group: &'p ManifestWriteGroup,
    },
    /// Mixed-routing manifests (written through both routes) are never batched: each write is
    /// its own load/persist, strictly in plan order.
    MixedManifestVersion {
        path: &'p PathBuf,
        bump: &'p PlannedBump,
    },
    MixedWorkspaceVersion {
        root_manifest: &'p PathBuf,
        to: &'p Version,
    },
    MixedManifestRewrite {
        path: &'p PathBuf,
        rewrite: &'p SpecRewrite,
    },
    MixedWorkspaceRewrite {
        root_manifest: &'p PathBuf,
        rewrite: &'p SpecRewrite,
    },
    PlatformVersion(&'p PlatformWrite),
    OptionalDependencies(&'p OptionalDepUpdate),
}

/// Orders every manifest write once, so execution is a single `match` and the per-file order
/// (bump, then rewrites, then one persist) is data rather than control flow.
fn write_steps<'p>(plan: &'p VersionPlan, classification: &'p ManifestWriteClassification) -> Vec<WriteStep<'p>> {
    let mut steps = Vec::new();
    for (path, group) in &classification.batched {
        steps.push(WriteStep::Manifest { path, group });
    }
    for (root_manifest, group) in &classification.resolver_batched {
        steps.push(WriteStep::WorkspaceRoot { root_manifest, group });
    }
    for bump in &plan.bumps {
        for write in &bump.writes {
            match write {
                VersionWriteTarget::Manifest(path) if classification.excluded.contains(path) => {
                    steps.push(WriteStep::MixedManifestVersion { path, bump });
                }
                VersionWriteTarget::CargoWorkspacePackage { root_manifest }
                    if !classification.resolver_batched.contains_key(root_manifest) =>
                {
                    steps.push(WriteStep::MixedWorkspaceVersion {
                        root_manifest,
                        to: &bump.to,
                    });
                }
                _ => {}
            }
        }
    }
    for rewrite in &plan.rewrites {
        match &rewrite.key.target {
            DepWriteTarget::Manifest(path) if classification.excluded.contains(path) => {
                steps.push(WriteStep::MixedManifestRewrite { path, rewrite });
            }
            DepWriteTarget::CargoWorkspaceDependency { root_manifest }
                if !classification.resolver_batched.contains_key(root_manifest) =>
            {
                steps.push(WriteStep::MixedWorkspaceRewrite { root_manifest, rewrite });
            }
            _ => {}
        }
    }
    steps.extend(plan.platform_writes.iter().map(WriteStep::PlatformVersion));
    steps.extend(plan.optional_dep_updates.iter().map(WriteStep::OptionalDependencies));
    steps
}

/// Performs one step and returns the path to stage.
fn apply_write_step(
    step: &WriteStep<'_>,
    plan: &VersionPlan,
    root: &Path,
    ctx: &OpenContext<'_>,
    permit: &ApplyPermit,
) -> Result<PathBuf, GraphError> {
    match step {
        WriteStep::Manifest { path, group } => {
            let mut handle = open_canonical(path, ctx)?;
            let mut mutated = false;
            if let Some((bump_idx, target_version)) = &group.bump {
                let bump = &plan.bumps[*bump_idx];
                mutated = write_version_guarded(handle.as_mut(), path, &bump.from, target_version, permit)?;
            }
            for rewrite_idx in &group.rewrite_indices {
                rewrite_dependency(handle.as_mut(), &plan.rewrites[*rewrite_idx], permit)?;
                mutated = true;
            }
            if mutated {
                handle.persist(permit)?;
            }
            Ok((*path).clone())
        }
        WriteStep::WorkspaceRoot { root_manifest, group } => {
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
            Ok((*root_manifest).clone())
        }
        WriteStep::MixedManifestVersion { path, bump } => {
            let mut handle = open_canonical(path, ctx)?;
            if write_version_guarded(handle.as_mut(), path, &bump.from, &bump.to, permit)? {
                handle.persist(permit)?;
            }
            Ok((*path).clone())
        }
        WriteStep::MixedWorkspaceVersion { root_manifest, to } => {
            let mut ws_res = WorkspaceCargoResolver::load(&root.join(root_manifest))?;
            ws_res.write_version(to, permit)?;
            ws_res.persist(permit)?;
            Ok((*root_manifest).clone())
        }
        WriteStep::MixedManifestRewrite { path, rewrite } => {
            let mut handle = open_canonical(path, ctx)?;
            rewrite_dependency(handle.as_mut(), rewrite, permit)?;
            handle.persist(permit)?;
            Ok((*path).clone())
        }
        WriteStep::MixedWorkspaceRewrite { root_manifest, rewrite } => {
            let mut ws_res = WorkspaceCargoResolver::load(&root.join(root_manifest))?;
            ws_res.write_dependency(&rewrite.key.name, rewrite.to.clone(), permit)?;
            ws_res.persist(permit)?;
            Ok((*root_manifest).clone())
        }
        WriteStep::PlatformVersion(write) => {
            let mut handle = open_canonical(&write.manifest, ctx)?;
            if write_version_guarded(handle.as_mut(), &write.manifest, &write.from, &write.version, permit)? {
                handle.persist(permit)?;
            }
            Ok(write.manifest.clone())
        }
        WriteStep::OptionalDependencies(update) => {
            let mut handle = open_canonical(&update.manifest, ctx)?;
            handle.update_optional_dependencies(&update.updates, permit)?;
            handle.persist(permit)?;
            Ok(update.manifest.clone())
        }
    }
}

fn open_canonical(path: &Path, ctx: &OpenContext<'_>) -> Result<Box<dyn Manifest>, GraphError> {
    let fmt = callisto_model::ManifestFormat::from_path(path)?;
    let decl = callisto_model::ManifestDecl::new(path, ManifestRole::Canonical, fmt)?;
    Ok(open(&decl, ctx)?)
}

/// Writes `to` unless the manifest already has it, and refuses any version other than `from`:
/// the precondition that makes re-running an interrupted apply safe. Returns whether it wrote.
fn write_version_guarded(
    handle: &mut dyn Manifest,
    path: &Path,
    from: &Version,
    to: &Version,
    permit: &ApplyPermit,
) -> Result<bool, GraphError> {
    let current = handle.current_version()?;
    if current == *to {
        return Ok(false);
    }
    if current != *from {
        return Err(GraphError::UnexpectedManifestVersion {
            path: path.to_path_buf(),
            expected_from: from.clone(),
            expected_to: to.clone(),
            found: current,
        });
    }
    handle.write_version(to, permit)?;
    Ok(true)
}

fn rewrite_dependency(
    handle: &mut dyn Manifest,
    rewrite: &SpecRewrite,
    permit: &ApplyPermit,
) -> Result<(), GraphError> {
    handle.update_dependency_spec(
        &rewrite.key.name,
        rewrite.key.kind.unwrap_or(DepKind::Runtime),
        &rewrite.key.locator,
        rewrite.to.clone(),
        permit,
    )?;
    Ok(())
}

/// The ecosystems a plan touches, so only their lockfiles are refreshed and staged. Staging
/// the lockfile of an untouched ecosystem could sweep up unrelated user changes.
fn active_ecosystems(plan: &VersionPlan) -> HashSet<Ecosystem> {
    let of_manifest = |path: &Path| ManifestFormat::from_path(path).ok().map(|fmt| fmt.ecosystem());
    let mut active = HashSet::new();
    for bump in &plan.bumps {
        active.extend(bump.package.ecosystem());
        for write in &bump.writes {
            active.extend(match write {
                VersionWriteTarget::CargoWorkspacePackage { .. } => Some(Ecosystem::Cargo),
                VersionWriteTarget::Manifest(path) => of_manifest(path),
            });
        }
    }
    for rewrite in &plan.rewrites {
        active.extend(match &rewrite.key.target {
            DepWriteTarget::CargoWorkspaceDependency { .. } => Some(Ecosystem::Cargo),
            DepWriteTarget::Manifest(path) => of_manifest(path),
        });
    }
    active.extend(
        plan.platform_writes
            .iter()
            .filter_map(|write| of_manifest(&write.manifest)),
    );
    active.extend(
        plan.optional_dep_updates
            .iter()
            .filter_map(|update| of_manifest(&update.manifest)),
    );
    active
}

/// Runs `git <prefix> <paths...>` in `root`, mapping a non-zero exit to `GraphError::Command`.
fn run_git_on_paths<R: CommandRunner>(
    runner: &R,
    root: &Path,
    prefix: &[&str],
    paths: &[&PathBuf],
) -> Result<(), GraphError> {
    let rendered: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    let mut args: Vec<&str> = prefix.to_vec();
    args.extend(rendered.iter().map(String::as_str));
    let output = runner.run("git", &args, root)?;
    if !output.success() {
        return Err(GraphError::Command(CommandError::Failed {
            program: "git".to_string(),
            exit_code: output.exit_code,
            stderr: output.redacted_stderr(),
        }));
    }
    Ok(())
}

/// Regenerates the lockfile of each active ecosystem with the ecosystem's own tool. For Python,
/// exactly one lockfile is refreshed, in uv, poetry, pdm order.
fn refresh_lockfiles<R: CommandRunner>(
    runner: &R,
    root: &Path,
    active_ecosystems: &HashSet<Ecosystem>,
) -> Result<Vec<LockfileRefreshResult>, GraphError> {
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

    Ok(refresh_results)
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
