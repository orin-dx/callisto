use callisto_model::{CommandRunner, SnapshotReport, SCHEMA_VERSION};
use callisto_vcs::GitDataSource;

use crate::error::GraphError;
use crate::plan::VersionPlan;
use crate::resolver::DependencyResolver;
use crate::Workspace;

pub fn plan_snapshot<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    tag: &str,
) -> Result<(VersionPlan, SnapshotReport), GraphError> {
    // §G.11 (SPEC DECISION, pinned invariant #33): the sha component is a real, resolved
    // HEAD commit sha — never a fake placeholder. A resolution failure here must surface
    // as a real error rather than silently proceeding with a value that risks colliding
    // with snapshots from unrelated runs. `ws.git_access()` (native gix, falling back to
    // the `CommandRunner` shell path when unavailable -- always true on wasm32) rather
    // than a direct `GitRepository::discover`, which has no such fallback and would
    // unconditionally fail on wasm32, hard-erroring `plan_snapshot` entirely there --
    // and reuses `Workspace`'s shared, lazily-discovered instance instead of paying for
    // a second discovery when something else in this command invocation already triggered one.
    let sha = ws.git_access().head_sha()?;
    let sha_short = sha.short();

    // Base is literally `0.0.0`, never the package's own version, and every package in
    // the workspace gets this identical, hyphen-joined string (§G.11 invariant #33) — not
    // a per-package, dot-joined prerelease of that package's real version.
    let snapshot_tag = format!("0.0.0-{tag}-{sha_short}");
    let snapshot_ver =
        callisto_model::Version::parse(&snapshot_tag, callisto_model::VersionGrammar::SemVer).map_err(|_err| {
            GraphError::Bump(callisto_format::BumpError::NotSemVer {
                raw: snapshot_tag.clone(),
                grammar: callisto_model::VersionGrammar::SemVer,
            })
        })?;
    let base_versions = ws.base_versions()?;
    let tags = ws.tags()?;
    let mut initial_severities = std::collections::BTreeMap::new();
    let mut initial_reasons = std::collections::BTreeMap::new();
    let mut initial_named_by = std::collections::BTreeMap::new();

    for pkg in ws.graph.packages() {
        initial_severities.insert(pkg.id.clone(), callisto_model::Severity::Patch);
        initial_reasons.insert(
            pkg.id.clone(),
            callisto_model::BumpReason::PreRelease { tag: tag.to_string() },
        );
        initial_named_by.insert(pkg.id.clone(), crate::aggregate::NamedBy::Changeset);
    }

    let cascade_input = crate::cascade::CascadeInput {
        graph: &ws.graph,
        groups: &ws.config.groups,
        cfg: &ws.config.cascade,
        seed: &initial_severities,
        reasons: &initial_reasons,
        named_by: &initial_named_by,
        base: &base_versions,
        pre: None,
        tags,
        identity: &ws.identity,
    };

    let cascade_out = crate::cascade::run_cascade(cascade_input)?;

    let mut bumps = Vec::new();
    let mut plan_bumps = Vec::new();

    let mut snapshot_versions = std::collections::BTreeMap::new();

    for pkg in ws.graph.packages() {
        let from = base_versions.get(&pkg.id).cloned().ok_or_else(|| {
            GraphError::Manifest(callisto_model::ManifestError::MissingField {
                path: pkg.manifests.first().map(|m| m.path.clone()).unwrap_or_default(),
                field: "version",
            })
        })?;
        snapshot_versions.insert(pkg.id.clone(), snapshot_ver.clone());

        let mut writes = Vec::new();
        for decl in &pkg.manifests {
            if decl.role == callisto_model::ManifestRole::Canonical {
                writes.push(crate::plan::VersionWriteTarget::Manifest(decl.path.clone()));
            }
        }

        plan_bumps.push(crate::plan::PlannedBump {
            package: pkg.id.clone(),
            from: from.clone(),
            to: snapshot_ver.clone(),
            severity: callisto_model::Severity::Patch,
            governed_by: None,
            reason: None,
            writes,
        });

        bumps.push(callisto_model::BumpRecord {
            package: pkg.id.clone(),
            from,
            to: snapshot_ver.clone(),
            severity: callisto_model::Severity::Patch,
            governed_by: None,
            reason: None,
        });
    }

    let mut rewrites: Vec<_> = cascade_out.rewrites.into_values().collect();
    for rewrite in &mut rewrites {
        if let Some(snap_to) = snapshot_versions.get(&rewrite.dependency) {
            let eco = rewrite
                .dependency
                .ecosystem()
                .unwrap_or(callisto_model::Ecosystem::Cargo);
            match crate::cascade::rewrite_spec(&rewrite.from, snap_to, eco, &ws.config.cascade) {
                crate::cascade::RewriteOutcome::Rewritten(new_spec) => {
                    rewrite.to = new_spec;
                }
                _ => {
                    rewrite.to = callisto_model::DepSpec::Exact(snap_to.clone());
                }
            }
        }
    }

    let plan = VersionPlan {
        bumps: plan_bumps,
        rewrites,
        platform_writes: Vec::new(),
        optional_dep_updates: Vec::new(),
        changelog_writes: Vec::new(),
        consumed_changesets: Vec::new(),
        pre_state_update: None,
        delete_pre_json: None,
        pre_cursor_updates: Vec::new(),
        observed_versions: std::collections::BTreeMap::new(),
        diagnostics: cascade_out.diagnostics,
    };

    let report = SnapshotReport {
        schema_version: SCHEMA_VERSION,
        snapshot_tag,
        bumps,
        diagnostics: Vec::new(),
    };

    Ok((plan, report))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::locate::IgnoreWalkLocator;
    use crate::Workspace;
    use callisto_model::{CommandError, CommandOutput, CommandRunner};

    use super::plan_snapshot;

    struct NoopSuccessRunner;

    impl CommandRunner for NoopSuccessRunner {
        fn run(&self, _program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    struct FailingTagsRunner;

    impl CommandRunner for FailingTagsRunner {
        fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
            if program == "git" && args == ["tag", "--list"] {
                return Err(CommandError::NotFound {
                    program: "git".to_string(),
                });
            }
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    fn git_init_with_commit(root: &Path) {
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "Test"],
            vec!["config", "commit.gpgsign", "false"],
            vec!["config", "tag.gpgsign", "false"],
        ] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(root)
                .output()
                .expect("git must be installed");
        }
        std::fs::write(root.join(".gitkeep"), "").unwrap();
        for args in [vec!["add", "."], vec!["commit", "-q", "-m", "init"]] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(root)
                .output()
                .expect("git must be installed");
        }
    }

    /// AC-014: when ws.tags() (TagIndex::build) fails, plan_snapshot must
    /// propagate that Err rather than silently proceeding with an
    /// empty/partial TagIndex.
    #[test]
    fn plan_snapshot_propagates_tags_build_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("pkg-alpha")).unwrap();
        std::fs::write(
            root.join("pkg-alpha/Cargo.toml"),
            "[package]\nname = \"pkg-alpha\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        assert!(
            callisto_vcs::GitRepository::discover(root).is_err(),
            "fixture must not be a discoverable git repo, forcing the CommandRunner fallback"
        );

        let locator = IgnoreWalkLocator::new(root);
        let runner = FailingTagsRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner)
            .expect("workspace must load even though git tags will later fail");

        let result = plan_snapshot(&ws, "canary");

        assert!(
            result.is_err(),
            "plan_snapshot must propagate a tags-build failure as Err, not silently proceed"
        );
    }

    /// AC-005: plan_snapshot's synthetic snapshot version overwrites every
    /// PlannedBump.to and BumpRecord.to unconditionally, even for members of
    /// a Fixed group -- the cascade-stage target the new Fixed-group
    /// convergence block computes internally is discarded.
    #[test]
    fn plan_snapshot_overwrites_fixed_group_targets_with_synthetic_version() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("pkg-a")).unwrap();
        std::fs::write(
            root.join("pkg-a/Cargo.toml"),
            "[package]\nname = \"pkg-a\"\nversion = \"2.0.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("pkg-b")).unwrap();
        std::fs::write(
            root.join("pkg-b/Cargo.toml"),
            "[package]\nname = \"pkg-b\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"ab\"\nmembers = [\"pkg-a\", \"pkg-b\"]\n",
        )
        .unwrap();

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopSuccessRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let (plan, report) = plan_snapshot(&ws, "canary").expect("plan_snapshot must succeed");

        let expected_prefix = "0.0.0-canary-";
        for bump in &plan.bumps {
            assert!(
                bump.to.render().starts_with(expected_prefix),
                "PlannedBump.to for {:?} must be the synthetic snapshot version, got {}",
                bump.package,
                bump.to.render()
            );
        }
        for bump in &report.bumps {
            assert!(
                bump.to.render().starts_with(expected_prefix),
                "BumpRecord.to for {:?} must be the synthetic snapshot version, got {}",
                bump.package,
                bump.to.render()
            );
        }
    }

    /// AC-006 (non-goal confirmation): plan_snapshot must never call
    /// pre_mutation_checks -- snapshot mode assigns every package the
    /// identical synthetic version, so there is no distinct-target
    /// divergence for pre_mutation_checks to ever catch here.
    #[test]
    fn plan_snapshot_source_never_calls_pre_mutation_checks() {
        let source = include_str!("snapshot.rs");
        // Only the production code above the `#[cfg(test)]` boundary is
        // checked -- this test module's own name and doc comments
        // necessarily mention `pre_mutation_checks` and must not cause a
        // self-match false failure.
        let production_source = source
            .split_once("#[cfg(test)]")
            .map(|(before, _)| before)
            .unwrap_or(source);
        assert!(
            !production_source.contains("pre_mutation_checks"),
            "plan_snapshot must not call pre_mutation_checks (see spec non_goals)"
        );
    }
}
