use std::collections::{HashMap, HashSet};

use callisto_changelog::{ChangeSource, ChangelogEntry, ChangelogInput};
use callisto_manifests::{open, OpenContext, WorkspaceCargoResolver};
use callisto_model::{BumpReason, CommandRunner, GroupKind, ManifestRole, Severity};

use crate::aggregate::aggregate;
use crate::cascade::{run_cascade, CascadeInput};
use crate::commands::escalate;
use crate::config::groups::{GroupMember, GroupMemberKind};
use crate::error::GraphError;
use crate::infer::SeverityInference;
use crate::plan::{PlannedBump, PlatformWrite, VersionPlan, VersionWriteTarget};
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct VersionOptions {
    pub strict: bool,
    pub strict_graph: bool,
    pub allow_empty_changesets: bool,
}

fn map_reason_to_change_source(
    reason: &BumpReason,
    id: &callisto_model::PackageId,
    to: &callisto_model::Version,
    outcome: &crate::cascade::CascadeOutcome,
    agg: &crate::aggregate::Aggregation,
) -> Option<ChangeSource> {
    match reason {
        BumpReason::FixedGroupUnion { group } => Some(ChangeSource::GroupUnion {
            group: group.clone(),
            kind: GroupKind::Fixed,
        }),
        BumpReason::LinkedGroupUnion { group } => Some(ChangeSource::GroupUnion {
            group: group.clone(),
            kind: GroupKind::Linked,
        }),
        BumpReason::Cascade {
            via,
            dep_kind,
            dependency_to,
            ..
        } => Some(ChangeSource::DependencyUpdate {
            dependency: via.clone(),
            dep_kind: *dep_kind,
            to: dependency_to.clone(),
        }),
        BumpReason::PeerEscalation { via, .. } => {
            let dep_to = outcome.targets.get(via).cloned().unwrap_or_else(|| to.clone());
            Some(ChangeSource::PeerEscalation {
                dependency: via.clone(),
                to: dep_to,
            })
        }
        BumpReason::Inference { commits, .. } => match agg.inference_commits.get(id).and_then(|c| c.first()) {
            Some((sha, subject)) => Some(ChangeSource::Commit {
                sha: sha.clone(),
                subject: subject.clone(),
            }),
            None => Some(ChangeSource::Changeset {
                filename: String::new(),
                summary: format!("Inferred version bump ({commits} commit(s))"),
            }),
        },
        _ => None,
    }
}

pub fn plan_version<R: CommandRunner, D: DependencyResolver, I: SeverityInference>(
    ws: &Workspace<'_, R, D>,
    inference: &I,
    opts: &VersionOptions,
) -> Result<VersionPlan, GraphError> {
    let base_versions = ws.base_versions()?;
    let tags = ws.tags()?;

    let pre_path = ws.root.join(".changeset/pre.json");
    let pre_state = if pre_path.exists() {
        let text =
            std::fs::read_to_string(&pre_path).map_err(|e| GraphError::PreJsonRead { message: e.to_string() })?;
        Some(callisto_format::parse_pre_json(&text).map_err(GraphError::PreJson)?)
    } else {
        None
    };

    let mut agg = aggregate(
        &ws.graph,
        &ws.config,
        ws.git_access(),
        tags,
        &base_versions,
        pre_state.as_ref(),
        inference,
    )?;

    // For PreMode::Exit, inject a synthetic Patch severity for every package
    // that is currently at a pre-release version. This triggers bump_target
    // (which in Exit mode calls versioning.bump on the pre-release, stripping
    // the tag and finalizing to stable) even when no pending changeset files
    // exist on disk — they were consumed and deleted during the pre-release phase
    // and their stems recorded in pre.json.
    if let Some(ref pre) = pre_state {
        if pre.mode == callisto_format::PreMode::Exit {
            for (id, ver) in &base_versions {
                if ver.is_prerelease() {
                    agg.severities.entry(id.clone()).or_insert(Severity::Patch);
                    agg.reasons
                        .entry(id.clone())
                        .or_insert(BumpReason::PreRelease { tag: pre.tag.clone() });
                }
            }
        }
    }

    let input = CascadeInput {
        graph: &ws.graph,
        groups: &ws.config.groups,
        cfg: &ws.config.cascade,
        seed: &agg.severities,
        reasons: &agg.reasons,
        named_by: &agg.named_by,
        base: &base_versions,
        pre: pre_state.as_ref(),
        tags,
        identity: &ws.identity,
    };

    let napi = crate::napi::NapiTargetsIndex::load(&ws.config.groups, &ws.root)?;
    let group_check =
        crate::groups::pre_mutation_checks(&ws.graph, &ws.config.groups, &base_versions, tags, &napi, &ws.root)?;

    let outcome = run_cascade(input)?;

    let mut bumps = Vec::new();
    let mut changelog_writes = Vec::new();

    // PERF-006: build a map once so each lookup inside the loop is O(1)
    // instead of O(N) (the previous packages().find() call).
    let pkg_map: HashMap<_, _> = ws.graph.packages().map(|p| (&p.id, p)).collect();

    for (id, &sev) in &outcome.severities {
        if sev == Severity::None {
            continue;
        }
        let pkg = pkg_map.get(id).copied().unwrap();
        let from = base_versions.get(id).cloned().ok_or_else(|| {
            GraphError::Manifest(callisto_model::ManifestError::MissingField {
                path: pkg.manifests.first().map(|m| m.path.clone()).unwrap_or_default(),
                field: "version",
            })
        })?;
        let to = outcome.targets.get(id).cloned().ok_or_else(|| {
            GraphError::Manifest(callisto_model::ManifestError::MissingField {
                path: pkg.manifests.first().map(|m| m.path.clone()).unwrap_or_default(),
                field: "version",
            })
        })?;

        let mut writes = Vec::new();
        for decl in &pkg.manifests {
            if decl.role == callisto_model::ManifestRole::Canonical {
                writes.push(VersionWriteTarget::Manifest(decl.path.clone()));
            }
        }

        bumps.push(PlannedBump {
            package: id.clone(),
            from: from.clone(),
            to: to.clone(),
            severity: sev,
            governed_by: outcome.governed_by.get(id).cloned(),
            reason: outcome.reasons.get(id).cloned(),
            writes,
        });

        if let Some(ch_path) = &pkg.changelog {
            let mut input = if let Some(mut agg_input) = agg.changelog_inputs.get(id).cloned() {
                // Real changeset data: use it, but set the resolved target version.
                agg_input.from = from.clone();
                agg_input.to = Some(to.clone());
                if let Some(reason) = outcome.reasons.get(id) {
                    if let Some(source) = map_reason_to_change_source(reason, id, &to, &outcome, &agg) {
                        agg_input.entries.push(ChangelogEntry { severity: sev, source });
                    }
                }
                agg_input
            } else {
                // No changeset drove this bump (cascade, group, inference, pre-release).
                // Synthesize a single entry describing the reason so that render_section()
                // never receives an empty entries list (which returns EmptyInput).
                let source = outcome
                    .reasons
                    .get(id)
                    .and_then(|r| map_reason_to_change_source(r, id, &to, &outcome, &agg))
                    .unwrap_or_else(|| ChangeSource::Changeset {
                        filename: String::new(),
                        summary: format!(
                            "Version bump ({})",
                            match sev {
                                Severity::Major => "major",
                                Severity::Minor => "minor",
                                Severity::Patch => "patch",
                                Severity::None => "none",
                            }
                        ),
                    });
                ChangelogInput {
                    package: id.clone(),
                    from: from.clone(),
                    to: Some(to.clone()),
                    entries: vec![ChangelogEntry { severity: sev, source }],
                }
            };

            if let Some(group_name) = group_check
                .new_members
                .iter()
                .find(|(_, members)| members.contains(id))
                .map(|(name, _)| name.clone())
            {
                input.entries.push(ChangelogEntry {
                    severity: sev,
                    source: ChangeSource::NewGroupMember { group: group_name },
                });
            }

            changelog_writes.push(crate::plan::ChangelogWrite {
                changelog_path: ch_path.clone(),
                input,
            });
        }
    }

    let mut diagnostics = outcome.diagnostics;
    diagnostics.extend(group_check.diagnostics);
    if agg.consumed.is_empty() && !opts.allow_empty_changesets && !ws.config.validation.allow_empty_changesets {
        diagnostics.push(callisto_model::Diagnostic {
            code: callisto_model::DiagnosticCode::EmptyChangeset,
            severity: callisto_model::DiagnosticSeverity::Warning,
            message: "No pending changesets found in workspace".to_string(),
            package: None,
            path: None,
            escalated_by: Some(callisto_model::StrictFlag::Strict),
            governed_by: Some(callisto_model::ConfigKey::VALIDATION_ALLOW_EMPTY_CHANGESETS),
        });
    }

    escalate(&mut diagnostics, opts.strict, opts.strict_graph);

    let (pre_state_update, delete_pre_json) = if let Some(mut state) = pre_state {
        if state.mode == callisto_format::PreMode::Exit {
            let rel_pre_path = ws.config.changesets_dir.join("pre.json");
            (None, Some(rel_pre_path))
        } else {
            // Update pre-release state changesets.
            // PERF-008: build an owned HashSet so each membership check is O(1)
            // instead of O(|state.changesets|); owned strings avoid the borrow
            // conflict that would arise from holding &str refs into the Vec
            // while also pushing to it.
            let seen: HashSet<String> = state.changesets.iter().cloned().collect();
            for cs in &agg.consumed {
                if let Some(name) = cs.file_name().and_then(|n| n.to_str()) {
                    let stem = name.strip_suffix(".md").unwrap_or(name).to_string();
                    if !seen.contains(&stem) {
                        state.changesets.push(stem);
                    }
                }
            }
            (Some(state), None)
        }
    } else {
        (None, None)
    };

    let bump_by_pkg: std::collections::BTreeMap<callisto_model::PackageId, &PlannedBump> =
        bumps.iter().map(|b| (b.package.clone(), b)).collect();

    let cargo_workspace = if ws.root.join("Cargo.toml").exists() {
        if let Ok(resolver) = WorkspaceCargoResolver::load(&ws.root.join("Cargo.toml")) {
            resolver.inheritance().ok().map(std::sync::Arc::new)
        } else {
            None
        }
    } else {
        None
    };
    let npm_workspace_kind = callisto_manifests::detect_npm_workspace_kind(&ws.root).ok().flatten();
    let open_ctx = OpenContext {
        workspace_root: &ws.root,
        cargo_workspace,
        npm_workspace_kind,
    };

    let mut platform_writes = Vec::new();
    let mut optional_dep_map: std::collections::BTreeMap<std::path::PathBuf, Vec<(String, callisto_model::Version)>> =
        std::collections::BTreeMap::new();
    for group in ws.config.groups.fixed.values() {
        for member in group.members(GroupMemberKind::PlatformManifest) {
            let GroupMember::PlatformManifest { owner, path, name, .. } = member else {
                continue;
            };
            let Some(bump) = bump_by_pkg.get(owner) else {
                continue;
            };
            let fmt = callisto_model::ManifestFormat::from_path(path)?;
            let decl = callisto_model::ManifestDecl::new(path.clone(), ManifestRole::Canonical, fmt)?;
            let handle = open(&decl, &open_ctx)?;
            let current = handle.current_version()?;
            platform_writes.push(PlatformWrite {
                manifest: path.clone(),
                version: bump.to.clone(),
                from: current,
            });

            if let Some(owner_pkg) = pkg_map.get(owner) {
                if let Some(owner_decl) = owner_pkg.canonical_manifests().next() {
                    let owner_fmt = callisto_model::ManifestFormat::from_path(&owner_decl.path)?;
                    let owner_manifest_decl =
                        callisto_model::ManifestDecl::new(owner_decl.path.clone(), ManifestRole::Canonical, owner_fmt)?;
                    let owner_handle = open(&owner_manifest_decl, &open_ctx)?;
                    let has_matching_optional_dep = owner_handle
                        .iter_dependencies()
                        .any(|dep| dep.kind == callisto_model::DepKind::Optional && &dep.name == name);
                    if has_matching_optional_dep {
                        optional_dep_map
                            .entry(owner_decl.path.clone())
                            .or_default()
                            .push((name.clone(), bump.to.clone()));
                    }
                }
            }
        }
    }
    let optional_dep_updates: Vec<crate::plan::OptionalDepUpdate> = optional_dep_map
        .into_iter()
        .map(|(manifest, updates)| crate::plan::OptionalDepUpdate { manifest, updates })
        .collect();

    Ok(VersionPlan {
        bumps,
        rewrites: outcome.rewrites.into_values().collect(),
        platform_writes,
        optional_dep_updates,
        changelog_writes,
        consumed_changesets: agg.consumed,
        pre_state_update,
        delete_pre_json,
        pre_cursor_updates: Vec::new(),
        observed_versions: base_versions,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{plan_version, VersionOptions};
    use crate::error::GraphError;
    use crate::infer::NoInference;
    use crate::locate::IgnoreWalkLocator;
    use crate::Workspace;
    use callisto_model::{CommandError, CommandOutput, CommandRunner};

    struct NoopRunner;

    impl CommandRunner for NoopRunner {
        fn run(&self, _program: &str, _args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
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

    fn tag(root: &Path, name: &str) {
        std::process::Command::new("git")
            .args(["-c", "tag.gpgSign=false", "tag", "-m", "release", name])
            .current_dir(root)
            .output()
            .expect("git must be installed");
    }

    fn commit_all(root: &Path, message: &str) {
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(root)
            .output()
            .expect("git add");
        std::process::Command::new("git")
            .args(["commit", "-q", "-m", message])
            .current_dir(root)
            .output()
            .expect("git commit");
    }

    /// AC-007 (divergent case) + AC-009 + AC-013(bug2): a Fixed group whose
    /// released members' base versions parse under the same grammar but
    /// disagree in value must make plan_version return
    /// Err(GraphError::FixedGroupDivergent) before run_cascade ever
    /// executes -- exercised through plan_version itself, not a direct
    /// pre_mutation_checks call.
    #[test]
    fn version_rejects_divergent_fixed_group_via_real_call_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        for (name, version) in [("pkg-a", "1.0.0"), ("pkg-b", "1.1.0")] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(
                root.join(name).join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\n"),
            )
            .unwrap();
        }
        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"ab\"\nmembers = [\"pkg-a\", \"pkg-b\"]\n",
        )
        .unwrap();
        commit_all(root, "add packages");
        tag(root, "pkg-a@1.0.0");
        tag(root, "pkg-b@1.1.0");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let result = plan_version(&ws, &inference, &opts);

        assert!(
            matches!(result, Err(GraphError::FixedGroupDivergent { .. })),
            "plan_version must return Err(FixedGroupDivergent) for a Fixed group with divergent released-member versions, got: {result:?}"
        );
    }

    /// AC-007 (grammar-mismatch case): a Fixed group whose released
    /// members use different version grammars (SemVer vs PEP 440) must
    /// return Err(GraphError::GroupGrammarMismatch) via plan_version's real
    /// call path, before run_cascade executes.
    #[test]
    fn version_rejects_grammar_mismatched_fixed_group_via_real_call_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("pkg-a")).unwrap();
        std::fs::write(
            root.join("pkg-a/Cargo.toml"),
            "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();

        std::fs::create_dir_all(root.join("pkg-py")).unwrap();
        std::fs::write(
            root.join("pkg-py/pyproject.toml"),
            "[project]\nname = \"pkg-py\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();

        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"mixed\"\nmembers = [\"pkg-a\", \"pkg-py\"]\n",
        )
        .unwrap();
        commit_all(root, "add packages");
        tag(root, "pkg-a@1.0.0");
        tag(root, "pkg-py@1.0.0");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let result = plan_version(&ws, &inference, &opts);

        assert!(
            matches!(result, Err(GraphError::GroupGrammarMismatch { .. })),
            "plan_version must return Err(GroupGrammarMismatch) for a Fixed group mixing SemVer and PEP 440 released members, got: {result:?}"
        );
    }

    /// AC-007b: a Fixed group's napi package.json existing at the
    /// conventional path but containing malformed JSON must surface as
    /// Err(GraphError::Manifest(ManifestError::Parse { .. })) from
    /// plan_version, with no VersionPlan produced.
    #[test]
    fn version_propagates_malformed_napi_package_json_as_manifest_parse_error() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("my-lib")).unwrap();
        std::fs::write(
            root.join("my-lib/Cargo.toml"),
            "[package]\nname = \"my-lib\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        // Truncated / malformed JSON at the conventional napi path.
        std::fs::write(
            root.join("my-lib/package.json"),
            "{\"name\":\"my-lib\",\"napi\":{\"targets\":[",
        )
        .unwrap();
        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"solo\"\nmembers = [\"my-lib\"]\n",
        )
        .unwrap();
        commit_all(root, "add package");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let result = plan_version(&ws, &inference, &opts);

        assert!(
            matches!(
                result,
                Err(GraphError::Manifest(callisto_model::ManifestError::Parse {
                    format: callisto_model::ManifestFormat::PackageJson,
                    ..
                }))
            ),
            "plan_version must propagate malformed napi package.json as Err(ManifestError::Parse); got: {result:?}"
        );
    }

    /// AC-008: GroupCheckOutcome.diagnostics returned by pre_mutation_checks
    /// must be merged into VersionPlan.diagnostics, not discarded.
    #[test]
    fn version_merges_group_check_diagnostics_into_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("my-lib")).unwrap();
        std::fs::write(
            root.join("my-lib/Cargo.toml"),
            "[package]\nname = \"my-lib\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        // Declares a napi target not present among the fixed group's own
        // members -- napi_drift reports NapiTargetAddedNotInMembers for this.
        std::fs::write(
            root.join("my-lib/package.json"),
            "{\"name\":\"my-lib\",\"napi\":{\"targets\":[\"aarch64-apple-darwin\"]}}",
        )
        .unwrap();
        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"solo\"\nmembers = [\"my-lib\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(root.join(".changeset/bump.md"), "---\n\"my-lib\": patch\n---\n\nfix.\n").unwrap();
        commit_all(root, "add package");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        assert!(
            plan.diagnostics
                .iter()
                .any(|d| matches!(d.code, callisto_model::DiagnosticCode::NapiTargetAddedNotInMembers)),
            "VersionPlan.diagnostics must include the napi-drift diagnostic produced by pre_mutation_checks; got: {:?}",
            plan.diagnostics
        );
    }

    /// AC-006: a package with a bump this run, `pkg.changelog` set, no pending
    /// changeset, and a BumpReason with no direct ChangeSource mapping
    /// (PreRelease) must still produce exactly one ChangelogWrite entry via
    /// the generic 'Version bump ({severity})' fallback -- plan_version must
    /// not panic or error.
    #[test]
    fn version_prerelease_exit_falls_back_to_generic_changeset_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("pkg-a")).unwrap();
        std::fs::write(
            root.join("pkg-a/Cargo.toml"),
            "[package]\nname = \"pkg-a\"\nversion = \"1.1.0-alpha.1\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(
            root.join(".changeset/pre.json"),
            r#"{"mode":"exit","tag":"alpha","initialVersions":{},"changesets":[]}"#,
        )
        .unwrap();
        commit_all(root, "add package");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        let pkg_a = callisto_model::PackageId::parse("pkg-a").unwrap();
        let write = plan
            .changelog_writes
            .iter()
            .find(|w| w.input.package == pkg_a)
            .expect("pkg-a must have a ChangelogWrite");

        assert_eq!(
            write.input.entries.len(),
            1,
            "expected exactly 1 entry, got {:?}",
            write.input.entries
        );
        match &write.input.entries[0].source {
            callisto_changelog::ChangeSource::Changeset { filename, summary } => {
                assert_eq!(filename, "");
                assert_eq!(summary, "Version bump (patch)");
            }
            other => panic!("expected ChangeSource::Changeset fallback, got {other:?}"),
        }
    }

    /// AC-001 + AC-002: a Fixed group with an owner Package member that
    /// receives a real PlannedBump this run, plus a sibling
    /// GroupMember::PlatformManifest member (a Case D hybrid-root npm
    /// package.json with non-empty os/cpu arrays living in the same
    /// directory as the owner's Cargo.toml), must produce exactly one
    /// PlatformWrite whose `manifest` is the platform manifest's path,
    /// whose `version` equals the owner's bump target, and whose `from`
    /// equals the platform manifest's on-disk current version at plan time.
    #[test]
    fn plan_version_emits_platform_write_for_bumped_owner_with_platform_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("crates/hybrid")).unwrap();
        std::fs::write(
            root.join("crates/hybrid/Cargo.toml"),
            "[package]\nname = \"hybrid\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/hybrid/package.json"),
            r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.9.0","os":["darwin"],"cpu":["arm64"]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"hybrid\", \"@myorg/hybrid-darwin-arm64\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(root.join(".changeset/bump.md"), "---\n\"hybrid\": patch\n---\n\nfix.\n").unwrap();
        commit_all(root, "add hybrid package");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        assert_eq!(
            plan.platform_writes.len(),
            1,
            "expected exactly one PlatformWrite, got: {:?}",
            plan.platform_writes
        );
        let pw = &plan.platform_writes[0];
        assert_eq!(
            pw.manifest,
            Path::new("crates/hybrid/package.json"),
            "PlatformWrite.manifest must be the platform manifest's path"
        );
        let owner_bump = plan
            .bumps
            .iter()
            .find(|b| b.package.name() == "hybrid")
            .expect("hybrid must have a planned bump");
        assert_eq!(
            pw.version, owner_bump.to,
            "PlatformWrite.version must equal the owner's bump target"
        );
        assert_eq!(
            pw.from.to_string(),
            "0.9.0",
            "PlatformWrite.from must equal the platform manifest's on-disk current version"
        );
    }

    /// AC-005: when no Fixed group anywhere has any
    /// GroupMember::PlatformManifest members, plan_version's
    /// platform_writes and optional_dep_updates must both be empty --
    /// no regression for workspaces without platform packages.
    #[test]
    fn plan_version_platform_writes_empty_when_no_platform_members_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("my-lib")).unwrap();
        std::fs::write(
            root.join("my-lib/Cargo.toml"),
            "[package]\nname = \"my-lib\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"solo\"\nmembers = [\"my-lib\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(root.join(".changeset/bump.md"), "---\n\"my-lib\": patch\n---\n\nfix.\n").unwrap();
        commit_all(root, "add package");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        assert!(
            plan.platform_writes.is_empty(),
            "platform_writes must be empty when no PlatformManifest members exist; got: {:?}",
            plan.platform_writes
        );
        assert!(
            plan.optional_dep_updates.is_empty(),
            "optional_dep_updates must be empty when no PlatformManifest members exist; got: {:?}",
            plan.optional_dep_updates
        );
    }

    /// AC-006: when a Fixed group's owner package receives no version bump
    /// this run, plan_version must produce no PlatformWrite for that
    /// group's GroupMember::PlatformManifest members, even though the
    /// platform manifest exists on disk -- platform writes are driven
    /// strictly by an actual owner PlannedBump, never speculatively.
    #[test]
    fn plan_version_no_platform_write_when_owner_not_bumped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("crates/hybrid")).unwrap();
        std::fs::write(
            root.join("crates/hybrid/Cargo.toml"),
            "[package]\nname = \"hybrid\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/hybrid/package.json"),
            r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.9.0","os":["darwin"],"cpu":["arm64"]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"hybrid\", \"@myorg/hybrid-darwin-arm64\"]\n",
        )
        .unwrap();
        // No changeset for "hybrid" -- outcome.severities for hybrid is
        // Severity::None, so no PlannedBump is produced for it.
        commit_all(root, "add hybrid package");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        assert!(
            plan.bumps.iter().all(|b| b.package.name() != "hybrid"),
            "hybrid must not have received a PlannedBump in this scenario; got bumps: {:?}",
            plan.bumps
        );
        assert!(
            plan.platform_writes.is_empty(),
            "platform_writes must be empty when the owner package received no bump; got: {:?}",
            plan.platform_writes
        );
    }

    /// AC-002b [REWRITTEN per corrected spec]: plan_version's
    /// OpenContext/workspace-inheritance resolution logic verified directly
    /// against a FIXTURE-CONSTRUCTED GroupMember::PlatformManifest (bypassing
    /// walk.rs discovery entirely, since walk.rs:199 only registers npm
    /// manifests into IdentityIndex.platform today -- no Cargo.toml can
    /// become a GroupMember::PlatformManifest through the real,
    /// disk-discovered call path). Confirms plan_version hardcodes
    /// ManifestRole::Canonical unconditionally (ignoring the fixture's
    /// Platform role) and correctly resolves a Cargo `version.workspace =
    /// true` manifest's inherited version from `[workspace.package]`.
    #[test]
    fn plan_version_platform_write_from_resolves_cargo_workspace_inherited_version() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        // Workspace root Cargo.toml supplying the [workspace.package] version
        // that a `version.workspace = true` platform manifest inherits from.
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\n\n[workspace.package]\nversion = \"3.2.1\"\n",
        )
        .unwrap();

        // The real owner package -- receives a real PlannedBump this run.
        std::fs::create_dir_all(root.join("crates/owner")).unwrap();
        std::fs::write(
            root.join("crates/owner/Cargo.toml"),
            "[package]\nname = \"cargo-owner\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();

        // The fixture's on-disk platform manifest target -- a maturin-style
        // Cargo.toml inheriting its version from [workspace.package]. This is
        // an ordinary, real, on-disk Cargo.toml; it is NOT registered as a
        // GroupMember::PlatformManifest by walk.rs (per AC-002b, that
        // classification is npm-only today) -- it is wired into the group
        // fixture manually below.
        std::fs::create_dir_all(root.join("platform/plat")).unwrap();
        std::fs::write(
            root.join("platform/plat/Cargo.toml"),
            "[package]\nname = \"cargo-owner-linux-x64-gnu\"\nversion.workspace = true\n",
        )
        .unwrap();

        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"cargo-owner-group\"\nmembers = [\"cargo-owner\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(
            root.join(".changeset/bump.md"),
            "---\n\"cargo-owner\": patch\n---\n\nfix.\n",
        )
        .unwrap();
        commit_all(root, "add owner and fixture platform crate");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let mut ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        // Fixture-construct the GroupMember::PlatformManifest, bypassing
        // walk.rs discovery entirely (per corrected AC-002b).
        let owner = callisto_model::PackageId::Bare("cargo-owner".to_string());
        let group_name = callisto_model::GroupName("cargo-owner-group".to_string());
        let group = ws
            .config
            .groups
            .fixed
            .get_mut(&group_name)
            .expect("cargo-owner-group must exist as a real Fixed group after Workspace::load");
        group
            .members
            .push(crate::config::groups::GroupMember::PlatformManifest {
                owner: owner.clone(),
                role: callisto_model::ManifestRole::Platform {
                    platform: "linux".to_string(),
                    arch: "x64".to_string(),
                    abi: Some("gnu".to_string()),
                },
                path: std::path::PathBuf::from("platform/plat/Cargo.toml"),
                name: "cargo-owner-linux-x64-gnu".to_string(),
            });

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        let owner_bump = plan
            .bumps
            .iter()
            .find(|b| b.package == owner)
            .expect("owner must receive a real PlannedBump this run, not a vacuous pass");

        assert_eq!(
            plan.platform_writes.len(),
            1,
            "expected exactly one PlatformWrite for the fixture-constructed Cargo platform member, got: {:?}",
            plan.platform_writes
        );
        let pw = &plan.platform_writes[0];
        assert_eq!(
            pw.manifest,
            Path::new("platform/plat/Cargo.toml"),
            "PlatformWrite.manifest must be the fixture platform manifest's path"
        );
        assert_eq!(
            pw.from.to_string(),
            "3.2.1",
            "PlatformWrite.from must resolve the Cargo workspace-inherited version.workspace = true \
             value from [workspace.package] version, not fail or silently default"
        );
        assert_eq!(
            pw.version, owner_bump.to,
            "PlatformWrite.version must equal the owner's real bump target"
        );
    }

    /// AC-007 (spec text names no discovery mechanism): an owner with two
    /// platform siblings (linux-x64-gnu, darwin-arm64), both bumped, must
    /// produce one `PlatformWrite` per sibling, not just the first.
    ///
    /// Linux is real/disk-discovered (Case D: `Cargo.toml`+`package.json`
    /// sharing a directory). Darwin is fixture-injected, because walk.rs's
    /// platform registration (walk.rs:131-227) groups strictly by directory
    /// -- a second platform manifest for the same owner can only be
    /// disk-discovered from that same directory, and a directory holds at
    /// most one `package.json`. So two real disk-discovered siblings under
    /// one owner isn't representable via `walk.rs` today (same constraint
    /// as AC-002b's Cargo correction). This test instead proves
    /// `plan_version`'s own loop -- agnostic to how a member entered
    /// `ws.config.groups.fixed` -- iterates every `PlatformManifest` member,
    /// not just the first.
    #[test]
    fn plan_version_emits_platform_write_for_each_of_two_platform_siblings() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        // Real, disk-discovered Case D linux sibling: Cargo.toml and
        // package.json sharing one directory, so `owner` resolves to the
        // Cargo primary_id via walk.rs's Case D promotion.
        std::fs::create_dir_all(root.join("crates/hybrid")).unwrap();
        std::fs::write(
            root.join("crates/hybrid/Cargo.toml"),
            "[package]\nname = \"hybrid\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/hybrid/package.json"),
            r#"{"name":"@myorg/hybrid-linux-x64-gnu","version":"0.9.0","os":["linux"],"cpu":["x64"]}"#,
        )
        .unwrap();

        // Second sibling's on-disk manifest target -- real content, read by
        // plan_version exactly as the first sibling's is, but fixture-wired
        // into the group below since walk.rs cannot discover a second
        // Case D member under the same owner.
        std::fs::create_dir_all(root.join("crates/hybrid-darwin")).unwrap();
        std::fs::write(
            root.join("crates/hybrid-darwin/package.json"),
            r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.5.0","os":["darwin"],"cpu":["arm64"]}"#,
        )
        .unwrap();

        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"hybrid\", \"@myorg/hybrid-linux-x64-gnu\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(root.join(".changeset/bump.md"), "---\n\"hybrid\": patch\n---\n\nfix.\n").unwrap();
        commit_all(root, "add hybrid package with linux platform sibling");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let mut ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let owner = callisto_model::PackageId::Bare("hybrid".to_string());
        let group_name = callisto_model::GroupName("hybrid-group".to_string());
        let group = ws
            .config
            .groups
            .fixed
            .get_mut(&group_name)
            .expect("hybrid-group must exist as a real Fixed group after Workspace::load");
        group
            .members
            .push(crate::config::groups::GroupMember::PlatformManifest {
                owner: owner.clone(),
                role: callisto_model::ManifestRole::Platform {
                    platform: "darwin".to_string(),
                    arch: "arm64".to_string(),
                    abi: None,
                },
                path: std::path::PathBuf::from("crates/hybrid-darwin/package.json"),
                name: "@myorg/hybrid-darwin-arm64".to_string(),
            });

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        let owner_bump = plan
            .bumps
            .iter()
            .find(|b| b.package == owner)
            .expect("hybrid must have a planned bump");

        assert_eq!(
            plan.platform_writes.len(),
            2,
            "expected one PlatformWrite per platform sibling (two total), got: {:?}",
            plan.platform_writes
        );

        let linux_pw = plan
            .platform_writes
            .iter()
            .find(|pw| pw.manifest == Path::new("crates/hybrid/package.json"))
            .expect("linux sibling must have its own PlatformWrite entry");
        assert_eq!(linux_pw.from.to_string(), "0.9.0");
        assert_eq!(linux_pw.version, owner_bump.to);

        let darwin_pw = plan
            .platform_writes
            .iter()
            .find(|pw| pw.manifest == Path::new("crates/hybrid-darwin/package.json"))
            .expect("darwin sibling must have its own PlatformWrite entry");
        assert_eq!(darwin_pw.from.to_string(), "0.5.0");
        assert_eq!(darwin_pw.version, owner_bump.to);
    }

    /// AC-007b: given the AC-007 scenario (owner with two
    /// `GroupMember::PlatformManifest` siblings, both bumped) where the
    /// owner's canonical manifest has matching `optionalDependencies`
    /// entries for both platform names, `plan_version`'s
    /// `VersionPlan.optional_dep_updates` must contain exactly one
    /// `OptionalDepUpdate` for the owner's canonical path, with both (name,
    /// version) pairs merged in -- never duplicated as separate entries
    /// for the same path.
    ///
    /// Fixture construction mirrors
    /// `plan_version_emits_platform_write_for_each_of_two_platform_siblings`
    /// (see that test's doc comment for why darwin is fixture-injected, not
    /// disk-discovered).
    #[test]
    fn plan_version_merges_two_platform_siblings_optional_dep_updates_into_one_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        // Real, disk-discovered Case D linux sibling: Cargo.toml and
        // package.json sharing one directory. The owner's Cargo.toml
        // declares matching `optional = true` dependencies on BOTH platform
        // members' npm names, so both siblings should produce a merged
        // OptionalDepUpdate entry rather than two separate ones.
        std::fs::create_dir_all(root.join("crates/hybrid")).unwrap();
        std::fs::write(
            root.join("crates/hybrid/Cargo.toml"),
            "[package]\nname = \"hybrid\"\nversion = \"1.0.0\"\n\n[dependencies]\n\"@myorg/hybrid-linux-x64-gnu\" = { version = \"0.9.0\", optional = true }\n\"@myorg/hybrid-darwin-arm64\" = { version = \"0.5.0\", optional = true }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/hybrid/package.json"),
            r#"{"name":"@myorg/hybrid-linux-x64-gnu","version":"0.9.0","os":["linux"],"cpu":["x64"]}"#,
        )
        .unwrap();

        // Second sibling's on-disk manifest target -- real content, read by
        // plan_version exactly as the first sibling's is, but fixture-wired
        // into the group below since walk.rs cannot discover a second
        // Case D member under the same owner.
        std::fs::create_dir_all(root.join("crates/hybrid-darwin")).unwrap();
        std::fs::write(
            root.join("crates/hybrid-darwin/package.json"),
            r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.5.0","os":["darwin"],"cpu":["arm64"]}"#,
        )
        .unwrap();

        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"hybrid\", \"@myorg/hybrid-linux-x64-gnu\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(root.join(".changeset/bump.md"), "---\n\"hybrid\": patch\n---\n\nfix.\n").unwrap();
        commit_all(root, "add hybrid package with linux platform sibling");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let mut ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let owner = callisto_model::PackageId::Bare("hybrid".to_string());
        let group_name = callisto_model::GroupName("hybrid-group".to_string());
        let group = ws
            .config
            .groups
            .fixed
            .get_mut(&group_name)
            .expect("hybrid-group must exist as a real Fixed group after Workspace::load");
        group
            .members
            .push(crate::config::groups::GroupMember::PlatformManifest {
                owner: owner.clone(),
                role: callisto_model::ManifestRole::Platform {
                    platform: "darwin".to_string(),
                    arch: "arm64".to_string(),
                    abi: None,
                },
                path: std::path::PathBuf::from("crates/hybrid-darwin/package.json"),
                name: "@myorg/hybrid-darwin-arm64".to_string(),
            });

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        let owner_bump = plan
            .bumps
            .iter()
            .find(|b| b.package == owner)
            .expect("hybrid must have a planned bump");

        assert_eq!(
            plan.optional_dep_updates.len(),
            1,
            "expected exactly one merged OptionalDepUpdate for the owner's canonical manifest, \
             not one per platform sibling; got: {:?}",
            plan.optional_dep_updates
        );
        let update = &plan.optional_dep_updates[0];
        assert_eq!(
            update.manifest,
            Path::new("crates/hybrid/Cargo.toml"),
            "OptionalDepUpdate.manifest must be the owner's canonical manifest path"
        );

        let mut updates = update.updates.clone();
        updates.sort_by(|a, b| a.0.cmp(&b.0));
        let mut expected = vec![
            ("@myorg/hybrid-linux-x64-gnu".to_string(), owner_bump.to.clone()),
            ("@myorg/hybrid-darwin-arm64".to_string(), owner_bump.to.clone()),
        ];
        expected.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            updates, expected,
            "OptionalDepUpdate.updates must contain both (name, version) pairs merged into the \
             single entry for the owner's manifest path"
        );
    }

    /// AC-008 (disjunct 1: missing file): a Fixed group's platform manifest
    /// that validated during the walk (so it is a real
    /// GroupMember::PlatformManifest) but has been deleted from disk before
    /// plan_version runs -- simulating the file having gone missing between
    /// workspace load and plan execution -- must surface as Err(GraphError),
    /// not panic and not produce a partial VersionPlan.
    #[test]
    fn plan_version_returns_err_when_platform_manifest_missing_from_disk_at_plan_time() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("crates/hybrid")).unwrap();
        std::fs::write(
            root.join("crates/hybrid/Cargo.toml"),
            "[package]\nname = \"hybrid\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let platform_manifest_path = root.join("crates/hybrid/package.json");
        std::fs::write(
            &platform_manifest_path,
            r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.9.0","os":["darwin"],"cpu":["arm64"]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"hybrid\", \"@myorg/hybrid-darwin-arm64\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(root.join(".changeset/bump.md"), "---\n\"hybrid\": patch\n---\n\nfix.\n").unwrap();
        commit_all(root, "add hybrid package");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        // Simulate the platform manifest going missing between workspace
        // load and plan_version execution.
        std::fs::remove_file(&platform_manifest_path).unwrap();

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let result = plan_version(&ws, &inference, &opts);

        assert!(
            result.is_err(),
            "plan_version must return Err when the platform manifest is missing from disk at plan time; got: {result:?}"
        );
    }

    /// AC-008 (disjunct 2: malformed content): a Fixed group's platform
    /// manifest that exists on disk but cannot be parsed as valid content
    /// for its ecosystem (malformed JSON) must also surface as
    /// Err(GraphError), not panic and not produce a partial VersionPlan --
    /// distinct from the missing-file disjunct above.
    #[test]
    fn plan_version_returns_err_when_platform_manifest_content_is_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("crates/hybrid")).unwrap();
        std::fs::write(
            root.join("crates/hybrid/Cargo.toml"),
            "[package]\nname = \"hybrid\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let platform_manifest_path = root.join("crates/hybrid/package.json");
        std::fs::write(
            &platform_manifest_path,
            r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.9.0","os":["darwin"],"cpu":["arm64"]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"hybrid\", \"@myorg/hybrid-darwin-arm64\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(root.join(".changeset/bump.md"), "---\n\"hybrid\": patch\n---\n\nfix.\n").unwrap();
        commit_all(root, "add hybrid package");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        // Simulate the platform manifest's content becoming malformed
        // (truncated, unbalanced JSON) between workspace load and
        // plan_version execution -- distinct from the file simply vanishing.
        std::fs::write(
            &platform_manifest_path,
            r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.9.0","#,
        )
        .unwrap();

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let result = plan_version(&ws, &inference, &opts);

        assert!(
            result.is_err(),
            "plan_version must return Err when the platform manifest content is malformed; got: {result:?}"
        );
    }

    /// AC-003: given the AC-001 scenario (owner bumped, sibling
    /// GroupMember::PlatformManifest with name N), and the owner's own
    /// canonical manifest already declares an optionalDependencies entry
    /// whose name exactly matches N, plan_version's
    /// VersionPlan.optional_dep_updates must contain an OptionalDepUpdate
    /// entry whose manifest is the owner's canonical manifest path and
    /// whose updates contains the pair (N, Y) where Y is the owner's bump
    /// target.
    #[test]
    fn plan_version_emits_optional_dep_update_when_owner_declares_matching_optional_dep() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("crates/hybrid")).unwrap();
        // Owner's canonical manifest declares an existing optional
        // dependency on the platform member's own npm name -- Cargo
        // supports `optional = true` deps, which parse as DepKind::Optional.
        std::fs::write(
            root.join("crates/hybrid/Cargo.toml"),
            "[package]\nname = \"hybrid\"\nversion = \"1.0.0\"\n\n[dependencies]\n\"@myorg/hybrid-darwin-arm64\" = { version = \"0.9.0\", optional = true }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/hybrid/package.json"),
            r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.9.0","os":["darwin"],"cpu":["arm64"]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"hybrid\", \"@myorg/hybrid-darwin-arm64\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(root.join(".changeset/bump.md"), "---\n\"hybrid\": patch\n---\n\nfix.\n").unwrap();
        commit_all(root, "add hybrid package");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        let owner_bump = plan
            .bumps
            .iter()
            .find(|b| b.package.name() == "hybrid")
            .expect("hybrid must have a planned bump");

        assert_eq!(
            plan.optional_dep_updates.len(),
            1,
            "expected exactly one OptionalDepUpdate, got: {:?}",
            plan.optional_dep_updates
        );
        let update = &plan.optional_dep_updates[0];
        assert_eq!(
            update.manifest,
            Path::new("crates/hybrid/Cargo.toml"),
            "OptionalDepUpdate.manifest must be the owner's canonical manifest path"
        );
        assert_eq!(
            update.updates,
            vec![("@myorg/hybrid-darwin-arm64".to_string(), owner_bump.to.clone())],
            "OptionalDepUpdate.updates must contain (N, Y) for the matching optional dependency"
        );
    }

    /// AC-004: given the AC-001 scenario, but the owner package's
    /// canonical manifest has no dependency entry (of any kind) whose name
    /// matches the platform member's name N, plan_version's
    /// VersionPlan.optional_dep_updates must contain no entry referencing
    /// N -- an OptionalDepUpdate is only produced when a matching
    /// optionalDependencies entry already exists, never speculatively
    /// created.
    #[test]
    fn plan_version_no_optional_dep_update_when_owner_has_no_matching_dependency() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("crates/hybrid")).unwrap();
        std::fs::write(
            root.join("crates/hybrid/Cargo.toml"),
            "[package]\nname = \"hybrid\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/hybrid/package.json"),
            r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.9.0","os":["darwin"],"cpu":["arm64"]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"hybrid-group\"\nmembers = [\"hybrid\", \"@myorg/hybrid-darwin-arm64\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(root.join(".changeset/bump.md"), "---\n\"hybrid\": patch\n---\n\nfix.\n").unwrap();
        commit_all(root, "add hybrid package");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        assert!(
            plan.optional_dep_updates.is_empty(),
            "optional_dep_updates must be empty when the owner's canonical manifest has no \
             matching dependency entry for the platform member's name; got: {:?}",
            plan.optional_dep_updates
        );
    }

    struct FixedInference {
        outcome: crate::infer::InferenceOutcome,
    }

    impl crate::infer::SeverityInference for FixedInference {
        fn infer(
            &self,
            _pkg: &callisto_model::Package,
            _git: &callisto_vcs::GitAccess<'_>,
            _window: crate::infer::InferenceWindowSpec<'_>,
        ) -> Result<Option<crate::infer::InferenceOutcome>, GraphError> {
            Ok(Some(self.outcome.clone()))
        }
    }

    /// AC-003 (non-empty commits): Inference-driven bump with a real,
    /// non-empty InferenceOutcome.commits must produce exactly one
    /// ChangelogEntry with ChangeSource::Commit from commits[0] (newest,
    /// since callisto-vcs shells `git log --no-merges` with no reversing
    /// flag -- git's default newest-first order).
    #[test]
    fn plan_version_inference_reason_maps_to_most_recent_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("pkg-a")).unwrap();
        std::fs::write(
            root.join("pkg-a/Cargo.toml"),
            "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        commit_all(root, "add package");

        let sha_recent = callisto_model::CommitSha::parse(&"a".repeat(40)).unwrap();
        let sha_older = callisto_model::CommitSha::parse(&"b".repeat(40)).unwrap();
        let inference = FixedInference {
            outcome: crate::infer::InferenceOutcome {
                severity: callisto_model::Severity::Minor,
                commit_count: 2,
                remapped: false,
                commits: vec![
                    (sha_recent.clone(), "feat: recent".to_string()),
                    (sha_older, "fix: older".to_string()),
                ],
            },
        };

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        let pkg_a = callisto_model::PackageId::parse("pkg-a").unwrap();
        let write = plan
            .changelog_writes
            .iter()
            .find(|w| w.input.package == pkg_a)
            .expect("pkg-a must have a ChangelogWrite");

        assert_eq!(write.input.entries.len(), 1);
        assert_eq!(write.input.entries[0].severity, callisto_model::Severity::Minor);
        match &write.input.entries[0].source {
            callisto_changelog::ChangeSource::Commit { sha, subject } => {
                assert_eq!(sha, &sha_recent);
                assert_eq!(subject, "feat: recent");
            }
            other => panic!("expected ChangeSource::Commit, got {other:?}"),
        }
    }

    /// AC-003 (empty commits): an InferenceOutcome with empty `commits` but
    /// nonzero `commit_count` must fall back to a ChangeSource::Changeset
    /// entry summarizing the count, mirroring AC-006's fallback pattern.
    #[test]
    fn plan_version_inference_reason_falls_back_when_commits_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("pkg-a")).unwrap();
        std::fs::write(
            root.join("pkg-a/Cargo.toml"),
            "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        commit_all(root, "add package");

        let inference = FixedInference {
            outcome: crate::infer::InferenceOutcome {
                severity: callisto_model::Severity::Minor,
                commit_count: 2,
                remapped: false,
                commits: vec![],
            },
        };

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        let pkg_a = callisto_model::PackageId::parse("pkg-a").unwrap();
        let write = plan
            .changelog_writes
            .iter()
            .find(|w| w.input.package == pkg_a)
            .expect("pkg-a must have a ChangelogWrite");

        assert_eq!(write.input.entries.len(), 1);
        match &write.input.entries[0].source {
            callisto_changelog::ChangeSource::Changeset { filename, summary } => {
                assert_eq!(filename, "");
                assert_eq!(summary, "Inferred version bump (2 commit(s))");
            }
            other => panic!("expected ChangeSource::Changeset fallback, got {other:?}"),
        }
    }

    /// AC-005 (Cascade sub-case): a package with a pending changeset AND a
    /// genuine Cascade reason this run must get its changeset entries
    /// followed by exactly one additional DependencyUpdate entry.
    #[test]
    fn plan_version_unions_changeset_with_cascade_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("pkg-core")).unwrap();
        std::fs::write(
            root.join("pkg-core/Cargo.toml"),
            "[package]\nname = \"pkg-core\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();

        std::fs::create_dir_all(root.join("pkg-app")).unwrap();
        std::fs::write(
            root.join("pkg-app/Cargo.toml"),
            "[package]\nname = \"pkg-app\"\nversion = \"1.0.0\"\n\n[dependencies]\npkg-core = \"1.0.0\"\n",
        )
        .unwrap();

        std::fs::write(root.join("callisto.toml"), "[cascade]\nbump-severity = \"minor\"\n").unwrap();

        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(
            root.join(".changeset/core-break.md"),
            "---\n\"pkg-core\": major\n---\n\nBreaking API change.\n",
        )
        .unwrap();
        std::fs::write(
            root.join(".changeset/app-patch.md"),
            "---\n\"pkg-app\": patch\n---\n\nUnrelated small fix.\n",
        )
        .unwrap();
        commit_all(root, "add packages and changesets");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");
        let inference = crate::infer::NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        let pkg_app = callisto_model::PackageId::parse("pkg-app").unwrap();
        let write = plan
            .changelog_writes
            .iter()
            .find(|w| w.input.package == pkg_app)
            .expect("pkg-app must have a ChangelogWrite");

        assert_eq!(write.input.entries.len(), 2, "got: {:?}", write.input.entries);
        assert!(matches!(
            write.input.entries[0].source,
            callisto_changelog::ChangeSource::Changeset { .. }
        ));
        assert!(matches!(
            write.input.entries[1].source,
            callisto_changelog::ChangeSource::DependencyUpdate { .. }
        ));
    }

    /// AC-005 (Inference sub-case, reusing T3's mapping): a package with
    /// both a pending changeset and outcome.reasons == Some(Inference{..})
    /// must get its changeset entries followed by one Inference-mapped
    /// entry (AC-003's Commit-or-fallback logic).
    #[test]
    fn plan_version_unions_changeset_with_inference_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        std::fs::create_dir_all(root.join("pkg-a")).unwrap();
        std::fs::write(
            root.join("pkg-a/Cargo.toml"),
            "[package]\nname = \"pkg-a\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(
            root.join(".changeset/pkg-a-patch.md"),
            "---\n\"pkg-a\": patch\n---\n\nSmall fix.\n",
        )
        .unwrap();
        commit_all(root, "add package and changeset");

        let sha = callisto_model::CommitSha::parse(&"c".repeat(40)).unwrap();
        let inference = FixedInference {
            outcome: crate::infer::InferenceOutcome {
                severity: callisto_model::Severity::Minor,
                commit_count: 1,
                remapped: false,
                commits: vec![(sha.clone(), "feat: bigger change".to_string())],
            },
        };

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        let pkg_a = callisto_model::PackageId::parse("pkg-a").unwrap();
        let write = plan
            .changelog_writes
            .iter()
            .find(|w| w.input.package == pkg_a)
            .expect("pkg-a must have a ChangelogWrite");

        assert_eq!(write.input.entries.len(), 2, "got: {:?}", write.input.entries);
        assert!(matches!(
            write.input.entries[0].source,
            callisto_changelog::ChangeSource::Changeset { .. }
        ));
        match &write.input.entries[1].source {
            callisto_changelog::ChangeSource::Commit { sha: got_sha, subject } => {
                assert_eq!(got_sha, &sha);
                assert_eq!(subject, "feat: bigger change");
            }
            other => panic!("expected ChangeSource::Commit, got {other:?}"),
        }
    }

    /// AC-004 (changeset-only sub-case): a fixed group with one released
    /// member and one fresh (never-tagged) member at the identical base
    /// version; the fresh member gets a changeset-driven bump with no other
    /// reason. Entries must be [Changeset, NewGroupMember] in that order.
    #[test]
    fn plan_version_appends_new_group_member_entry_after_changeset() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        for name in ["pkg-released", "pkg-fresh"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(
                root.join(name).join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
            )
            .unwrap();
        }
        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"grp\"\nmembers = [\"pkg-released\", \"pkg-fresh\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(
            root.join(".changeset/fresh-patch.md"),
            "---\n\"pkg-fresh\": patch\n---\n\nfix.\n",
        )
        .unwrap();
        commit_all(root, "add packages");
        tag(root, "pkg-released@1.0.0");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");
        let inference = crate::infer::NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        let pkg_fresh = callisto_model::PackageId::parse("pkg-fresh").unwrap();
        let write = plan
            .changelog_writes
            .iter()
            .find(|w| w.input.package == pkg_fresh)
            .expect("pkg-fresh must have a ChangelogWrite");

        assert_eq!(write.input.entries.len(), 2, "got: {:?}", write.input.entries);
        assert!(matches!(
            write.input.entries[0].source,
            callisto_changelog::ChangeSource::Changeset { .. }
        ));
        assert!(matches!(
            write.input.entries[1].source,
            callisto_changelog::ChangeSource::NewGroupMember { .. }
        ));
    }

    /// AC-004 (additive-on-top-of-AC-005 sub-case): same fixed-group
    /// fixture, but pkg-fresh is ALSO a cascade target this run (via a
    /// third driver package), so it has both a changeset entry and a
    /// cascade-mapped entry (T4) before the NewGroupMember append.
    #[test]
    fn plan_version_new_group_member_entry_is_additive_after_cascade_union() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        git_init_with_commit(root);

        for name in ["pkg-released", "pkg-fresh"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(
                root.join(name).join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
            )
            .unwrap();
        }
        std::fs::create_dir_all(root.join("pkg-driver")).unwrap();
        std::fs::write(
            root.join("pkg-driver/Cargo.toml"),
            "[package]\nname = \"pkg-driver\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("pkg-fresh/Cargo.toml"),
            "[package]\nname = \"pkg-fresh\"\nversion = \"1.0.0\"\n\n[dependencies]\npkg-driver = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("callisto.toml"),
            "[[fixed-group]]\nname = \"grp\"\nmembers = [\"pkg-released\", \"pkg-fresh\"]\n\n[cascade]\nbump-severity = \"minor\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".changeset")).unwrap();
        std::fs::write(
            root.join(".changeset/fresh-patch.md"),
            "---\n\"pkg-fresh\": patch\n---\n\nfix.\n",
        )
        .unwrap();
        std::fs::write(
            root.join(".changeset/driver-major.md"),
            "---\n\"pkg-driver\": major\n---\n\nBreaking.\n",
        )
        .unwrap();
        commit_all(root, "add packages");
        tag(root, "pkg-released@1.0.0");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws = Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");
        let inference = crate::infer::NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        let pkg_fresh = callisto_model::PackageId::parse("pkg-fresh").unwrap();
        let write = plan
            .changelog_writes
            .iter()
            .find(|w| w.input.package == pkg_fresh)
            .expect("pkg-fresh must have a ChangelogWrite");

        assert_eq!(write.input.entries.len(), 3, "got: {:?}", write.input.entries);
        assert!(matches!(
            write.input.entries[0].source,
            callisto_changelog::ChangeSource::Changeset { .. }
        ));
        assert!(matches!(
            write.input.entries[1].source,
            callisto_changelog::ChangeSource::DependencyUpdate { .. }
        ));
        assert!(matches!(
            write.input.entries[2].source,
            callisto_changelog::ChangeSource::NewGroupMember { .. }
        ));
    }
}
