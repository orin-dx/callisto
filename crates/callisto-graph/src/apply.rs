use std::fs;
use std::path::{Path, PathBuf};

use callisto_manifests::{open, OpenContext, WorkspaceCargoResolver};
use callisto_model::{
    ApplyPermit, CommandError, CommandOutput, CommandRunner, LockfileRefreshResult, ManifestRole,
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
    opts: &ApplyOptions,
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
                    let current = handle.current_version()?;
                    if current == bump.to {
                        // Already at target — skip write but still stage so git add re-stages on retry.
                    } else if current == bump.from {
                        handle.write_version(&bump.to, permit)?;
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
        }
        modified_paths.push(cs_path.clone());
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
    } else if let Some(rel_pre_path) = &plan.delete_pre_json {
        let pre_path = root.join(rel_pre_path);
        if pre_path.exists() {
            fs::remove_file(&pre_path).map_err(|e| {
                GraphError::Command(CommandError::Io {
                    program: "fs".to_string(),
                    message: e.to_string(),
                })
            })?;
            modified_paths.push(rel_pre_path.clone());
        }
    }

    // Collect the set of ecosystems actively involved in this plan so that
    // only the corresponding lockfiles are staged. Staging lockfiles whose
    // ecosystem was not touched can sweep up unrelated user changes.
    use callisto_model::Ecosystem;
    let active_ecosystems: std::collections::HashSet<Ecosystem> = plan
        .bumps
        .iter()
        .filter_map(|b| {
            // Prefer the ecosystem declared in the PackageId; fall back to
            // inferring it from write targets for Bare (unprefixed) ids.
            if let Some(eco) = b.package.ecosystem() {
                return Some(eco);
            }
            // Bare id: derive ecosystem from the write targets.
            for write in &b.writes {
                let eco = match write {
                    VersionWriteTarget::CargoWorkspacePackage { .. } => Ecosystem::Cargo,
                    VersionWriteTarget::Manifest(p) => {
                        match callisto_model::ManifestFormat::from_path(p) {
                            Ok(fmt) => fmt.ecosystem(),
                            Err(_) => continue,
                        }
                    }
                };
                return Some(eco);
            }
            None
        })
        .collect();

    // Regenerate lockfiles when the caller requested a refresh. This must run
    // BEFORE the git-staging loop so the refreshed files are on disk when they
    // are picked up by the staging pass below.
    if opts.refresh_lockfiles {
        let mut refresh_results: Vec<LockfileRefreshResult> = Vec::new();

        if active_ecosystems.contains(&Ecosystem::Cargo) {
            let out = runner
                .run("cargo", &["update", "--workspace"], root)
                .unwrap_or_else(|e| CommandOutput {
                    exit_code: None,
                    stdout: String::new(),
                    stderr: e.to_string(),
                });
            refresh_results.push(LockfileRefreshResult {
                filename: PathBuf::from("Cargo.lock"),
                refresh_command: "cargo update --workspace".to_string(),
                success: out.success(),
                exit_code: out.exit_code,
            });
        }

        if active_ecosystems.contains(&Ecosystem::Pypi) {
            if root.join("uv.lock").exists() {
                let out = runner
                    .run("uv", &["lock"], root)
                    .unwrap_or_else(|e| CommandOutput {
                        exit_code: None,
                        stdout: String::new(),
                        stderr: e.to_string(),
                    });
                refresh_results.push(LockfileRefreshResult {
                    filename: PathBuf::from("uv.lock"),
                    refresh_command: "uv lock".to_string(),
                    success: out.success(),
                    exit_code: out.exit_code,
                });
            } else if root.join("poetry.lock").exists() {
                let out = runner
                    .run("poetry", &["lock", "--no-update"], root)
                    .unwrap_or_else(|e| CommandOutput {
                        exit_code: None,
                        stdout: String::new(),
                        stderr: e.to_string(),
                    });
                refresh_results.push(LockfileRefreshResult {
                    filename: PathBuf::from("poetry.lock"),
                    refresh_command: "poetry lock --no-update".to_string(),
                    success: out.success(),
                    exit_code: out.exit_code,
                });
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use callisto_model::{
        ApplyPermit, CommandError, CommandOutput, CommandRunner, PackageId, Severity, Version,
        VersionGrammar,
    };

    use super::*;
    use crate::plan::{PlannedBump, VersionPlan};

    /// A no-op runner that always reports success for any git command.
    struct NoopRunner;

    impl CommandRunner for NoopRunner {
        fn run(
            &self,
            _program: &str,
            _args: &[&str],
            _cwd: &Path,
        ) -> Result<CommandOutput, CommandError> {
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    type CallLog = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<String>)>>>;

    /// Records every (program, args) pair that CommandRunner::run is called with.
    struct RecordingRunner {
        #[allow(clippy::type_complexity)]
        calls: CallLog,
    }

    impl CommandRunner for RecordingRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &Path,
        ) -> Result<CommandOutput, CommandError> {
            self.calls.lock().unwrap().push((
                program.to_string(),
                args.iter().map(|s| s.to_string()).collect(),
            ));
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    fn cargo_version(v: &str) -> Version {
        Version::parse(v, VersionGrammar::SemVer).expect("valid semver")
    }

    /// When only a Cargo package is bumped, the Python lockfile (`uv.lock`)
    /// must NOT appear in `staged`, even when it exists on disk alongside
    /// `Cargo.lock`.
    #[test]
    fn cargo_only_bump_does_not_stage_python_lockfile() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let root = dir.path();

        // Place both a Cargo lockfile and a Python lockfile on disk.
        std::fs::write(root.join("Cargo.lock"), "# fake Cargo.lock").unwrap();
        std::fs::write(root.join("uv.lock"), "# fake uv.lock").unwrap();

        let plan = VersionPlan {
            bumps: vec![PlannedBump {
                package: PackageId::parse("cargo:my-crate").expect("valid package id"),
                from: cargo_version("1.0.0"),
                to: cargo_version("1.1.0"),
                severity: Severity::Minor,
                governed_by: None,
                reason: None,
                writes: vec![], // no manifest writes — keeps test self-contained
            }],
            ..Default::default()
        };

        let permit = ApplyPermit::force_for_tests();
        let opts = ApplyOptions::default();
        let outcome = apply_version_plan(root, &plan, &NoopRunner, &opts, &permit)
            .expect("apply_version_plan should succeed");

        let staged_names: Vec<&str> = outcome.staged.iter().filter_map(|p| p.to_str()).collect();

        assert!(
            staged_names.contains(&"Cargo.lock"),
            "Cargo.lock should be staged when a Cargo package is bumped, got: {staged_names:?}"
        );
        assert!(
            !staged_names.contains(&"uv.lock"),
            "uv.lock must NOT be staged when no Python package is bumped, got: {staged_names:?}"
        );
    }

    /// When `refresh_lockfiles: true` and the plan bumps a Cargo package,
    /// `cargo update --workspace` must be called so that `Cargo.lock` is
    /// regenerated after the version bump. This prevents `cargo publish --locked`
    /// from failing with "lock file needs to be updated but --locked was passed".
    ///
    /// The result must appear in `ApplyOutcome::lockfile_refresh_results`.
    #[test]
    fn refresh_lockfiles_calls_cargo_update_workspace_when_cargo_bumped() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let root = dir.path();
        std::fs::write(root.join("Cargo.lock"), "# stale lock").unwrap();

        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let runner = RecordingRunner {
            calls: std::sync::Arc::clone(&calls),
        };

        let plan = VersionPlan {
            bumps: vec![PlannedBump {
                package: PackageId::parse("cargo:my-crate").expect("valid package id"),
                from: cargo_version("1.0.0"),
                to: cargo_version("1.1.0"),
                severity: Severity::Minor,
                governed_by: None,
                reason: None,
                writes: vec![],
            }],
            ..Default::default()
        };

        let permit = ApplyPermit::force_for_tests();
        let opts = ApplyOptions {
            refresh_lockfiles: true,
        };
        let outcome = apply_version_plan(root, &plan, &runner, &opts, &permit)
            .expect("apply_version_plan should succeed");

        let recorded = calls.lock().unwrap().clone();
        let cargo_update_called = recorded
            .iter()
            .any(|(prog, args)| prog == "cargo" && args.iter().any(|a| a == "update"));
        assert!(
            cargo_update_called,
            "cargo update must be called when refresh_lockfiles=true and Cargo package is bumped; calls: {recorded:?}"
        );

        let refresh_results = outcome
            .lockfile_refresh_results
            .expect("lockfile_refresh_results must be Some when refresh ran");
        assert!(
            refresh_results
                .iter()
                .any(|r| r.filename.as_os_str() == "Cargo.lock"),
            "Cargo.lock must appear in lockfile_refresh_results; got: {refresh_results:?}"
        );
    }

    /// When `refresh_lockfiles: false` (the default), `cargo update` must NOT
    /// be called — callers that do not request a refresh should see no extra
    /// subprocess invocations.
    #[test]
    fn refresh_lockfiles_false_does_not_call_cargo_update() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let root = dir.path();
        std::fs::write(root.join("Cargo.lock"), "# lock").unwrap();

        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let runner = RecordingRunner {
            calls: std::sync::Arc::clone(&calls),
        };

        let plan = VersionPlan {
            bumps: vec![PlannedBump {
                package: PackageId::parse("cargo:my-crate").expect("valid package id"),
                from: cargo_version("1.0.0"),
                to: cargo_version("1.1.0"),
                severity: Severity::Minor,
                governed_by: None,
                reason: None,
                writes: vec![],
            }],
            ..Default::default()
        };

        let permit = ApplyPermit::force_for_tests();
        let opts = ApplyOptions::default(); // refresh_lockfiles: false
        let outcome = apply_version_plan(root, &plan, &runner, &opts, &permit)
            .expect("apply_version_plan should succeed");

        let recorded = calls.lock().unwrap().clone();
        let cargo_update_called = recorded
            .iter()
            .any(|(prog, args)| prog == "cargo" && args.iter().any(|a| a == "update"));
        assert!(
            !cargo_update_called,
            "cargo update must NOT be called when refresh_lockfiles=false; calls: {recorded:?}"
        );
        assert!(
            outcome.lockfile_refresh_results.is_none(),
            "lockfile_refresh_results must be None when refresh_lockfiles=false"
        );
    }

    /// When a manifest is already at the plan's target version (from a prior
    /// partially-applied run that crashed), `apply_version_plan` must NOT bump
    /// it again (which would produce an unplanned 1.1.0 → 1.2.0 bump) and must
    /// return `Ok` so the caller can complete the remaining operations safely.
    ///
    /// The manifest write is skipped, but the path is still pushed to staged so
    /// that `git add` re-stages any changes that were made in the prior run.
    #[test]
    fn apply_is_idempotent_when_manifest_already_at_target_version() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let root = dir.path();

        // Manifest already at 1.1.0 — simulates a prior crashed apply.
        let cargo_toml_path = root.join("Cargo.toml");
        std::fs::write(
            &cargo_toml_path,
            "[package]\nname = \"my-crate\"\nversion = \"1.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        // Changeset file that would be consumed.
        let changeset_dir = root.join(".changeset");
        std::fs::create_dir_all(&changeset_dir).unwrap();
        std::fs::write(
            changeset_dir.join("my-change.md"),
            "---\nmy-crate: minor\n---\n",
        )
        .unwrap();

        let manifest_rel = PathBuf::from("Cargo.toml");
        let cs_rel = PathBuf::from(".changeset/my-change.md");

        let plan = VersionPlan {
            bumps: vec![PlannedBump {
                package: PackageId::parse("cargo:my-crate").expect("valid id"),
                from: cargo_version("1.0.0"),
                to: cargo_version("1.1.0"),
                severity: Severity::Minor,
                governed_by: None,
                reason: None,
                writes: vec![VersionWriteTarget::Manifest(manifest_rel.clone())],
            }],
            consumed_changesets: vec![cs_rel.clone()],
            ..Default::default()
        };

        let permit = ApplyPermit::force_for_tests();
        let opts = ApplyOptions::default();
        let outcome = apply_version_plan(root, &plan, &NoopRunner, &opts, &permit)
            .expect("apply_version_plan must succeed when manifest is already at target version");

        // Manifest must stay at 1.1.0, not be bumped to 1.2.0.
        let content = std::fs::read_to_string(&cargo_toml_path).unwrap();
        assert!(
            content.contains("version = \"1.1.0\""),
            "manifest must remain at 1.1.0 after idempotent apply; content: {content}"
        );

        // Changeset file must be deleted.
        assert!(
            !root.join(&cs_rel).exists(),
            "changeset file must be deleted even on an idempotent apply"
        );

        // Manifest must still be staged (re-add for git).
        assert!(
            outcome.staged.contains(&manifest_rel),
            "Cargo.toml must be in staged even when the write was skipped; staged: {:?}",
            outcome.staged
        );

        // Changeset path must be staged for git rm.
        assert!(
            outcome.staged.contains(&cs_rel),
            "changeset path must be in staged so git rm --cached runs; staged: {:?}",
            outcome.staged
        );
    }

    /// When a manifest is at a version that is neither `from` nor `to`,
    /// `apply_version_plan` must return an error — the workspace is in an
    /// unexpected state that requires human intervention.
    #[test]
    fn apply_returns_error_when_manifest_has_unexpected_version() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let root = dir.path();

        // Manifest at 2.0.0 — neither from=1.0.0 nor to=1.1.0.
        let cargo_toml_path = root.join("Cargo.toml");
        std::fs::write(
            &cargo_toml_path,
            "[package]\nname = \"my-crate\"\nversion = \"2.0.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        let manifest_rel = PathBuf::from("Cargo.toml");
        let plan = VersionPlan {
            bumps: vec![PlannedBump {
                package: PackageId::parse("cargo:my-crate").expect("valid id"),
                from: cargo_version("1.0.0"),
                to: cargo_version("1.1.0"),
                severity: Severity::Minor,
                governed_by: None,
                reason: None,
                writes: vec![VersionWriteTarget::Manifest(manifest_rel.clone())],
            }],
            ..Default::default()
        };

        let permit = ApplyPermit::force_for_tests();
        let opts = ApplyOptions::default();
        let result = apply_version_plan(root, &plan, &NoopRunner, &opts, &permit);

        assert!(
            matches!(result, Err(GraphError::UnexpectedManifestVersion { .. })),
            "apply must fail with UnexpectedManifestVersion when manifest is at an unexpected \
             version; got: {result:?}"
        );

        // Manifest must be unchanged.
        let content = std::fs::read_to_string(&cargo_toml_path).unwrap();
        assert!(
            content.contains("version = \"2.0.0\""),
            "manifest must not be modified when apply fails; content: {content}"
        );
    }

    /// When a changeset file was already deleted by a prior partial run,
    /// `apply_version_plan` must still push its path to `staged` so that
    /// `git rm --cached --ignore-unmatch` is called for it. Without this,
    /// a crashed-then-retried apply leaves the changeset in the git index.
    #[test]
    fn apply_stages_changeset_path_even_when_file_already_deleted() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let root = dir.path();

        // Manifest already at 1.1.0 (prior run bumped it).
        let cargo_toml_path = root.join("Cargo.toml");
        std::fs::write(
            &cargo_toml_path,
            "[package]\nname = \"my-crate\"\nversion = \"1.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        // Changeset file does NOT exist — deleted by the prior partial run.
        let cs_rel = PathBuf::from(".changeset/deleted-change.md");
        assert!(
            !root.join(&cs_rel).exists(),
            "changeset file must not exist at test start"
        );

        let manifest_rel = PathBuf::from("Cargo.toml");
        let plan = VersionPlan {
            bumps: vec![PlannedBump {
                package: PackageId::parse("cargo:my-crate").expect("valid id"),
                from: cargo_version("1.0.0"),
                to: cargo_version("1.1.0"),
                severity: Severity::Minor,
                governed_by: None,
                reason: None,
                writes: vec![VersionWriteTarget::Manifest(manifest_rel.clone())],
            }],
            consumed_changesets: vec![cs_rel.clone()],
            ..Default::default()
        };

        let permit = ApplyPermit::force_for_tests();
        let opts = ApplyOptions::default();
        let outcome = apply_version_plan(root, &plan, &NoopRunner, &opts, &permit)
            .expect("apply_version_plan must succeed");

        // Changeset path must be in staged regardless of whether the file existed.
        assert!(
            outcome.staged.contains(&cs_rel),
            "changeset path must be in staged even when file is already deleted; staged: {:?}",
            outcome.staged
        );
    }
}
