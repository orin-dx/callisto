use std::collections::{HashMap, HashSet};

use callisto_changelog::{ChangeSource, ChangelogEntry, ChangelogInput};
use callisto_model::{BumpReason, CommandRunner, GroupKind, Severity};

use crate::aggregate::aggregate;
use crate::cascade::{run_cascade, CascadeInput};
use crate::commands::escalate;
use crate::error::GraphError;
use crate::infer::SeverityInference;
use crate::plan::{PlannedBump, VersionPlan, VersionWriteTarget};
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct VersionOptions {
    pub strict: bool,
    pub strict_graph: bool,
    pub allow_empty_changesets: bool,
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
        let text = std::fs::read_to_string(&pre_path).map_err(|e| GraphError::PreJsonRead {
            message: e.to_string(),
        })?;
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
                        .or_insert(BumpReason::PreRelease {
                            tag: pre.tag.clone(),
                        });
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
    };

    let napi = crate::napi::NapiTargetsIndex::load(&ws.config.groups, &ws.root)?;
    let group_check = crate::groups::pre_mutation_checks(
        &ws.graph,
        &ws.config.groups,
        &base_versions,
        tags,
        &napi,
        &ws.root,
    )?;

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
                path: pkg
                    .manifests
                    .first()
                    .map(|m| m.path.clone())
                    .unwrap_or_default(),
                field: "version",
            })
        })?;
        let to = outcome.targets.get(id).cloned().ok_or_else(|| {
            GraphError::Manifest(callisto_model::ManifestError::MissingField {
                path: pkg
                    .manifests
                    .first()
                    .map(|m| m.path.clone())
                    .unwrap_or_default(),
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
            let input = if let Some(mut agg_input) = agg.changelog_inputs.get(id).cloned() {
                // Real changeset data: use it, but set the resolved target version.
                agg_input.from = from.clone();
                agg_input.to = Some(to.clone());
                agg_input
            } else {
                // No changeset drove this bump (cascade, group, inference, pre-release).
                // Synthesize a single entry describing the reason so that render_section()
                // never receives an empty entries list (which returns EmptyInput).
                let source = match outcome.reasons.get(id) {
                    Some(BumpReason::FixedGroupUnion { group }) => ChangeSource::GroupUnion {
                        group: group.clone(),
                        kind: GroupKind::Fixed,
                    },
                    Some(BumpReason::LinkedGroupUnion { group }) => ChangeSource::GroupUnion {
                        group: group.clone(),
                        kind: GroupKind::Linked,
                    },
                    Some(BumpReason::Cascade {
                        via,
                        dep_kind,
                        dependency_to,
                        ..
                    }) => ChangeSource::DependencyUpdate {
                        dependency: via.clone(),
                        dep_kind: *dep_kind,
                        to: dependency_to.clone(),
                    },
                    Some(BumpReason::PeerEscalation { via, .. }) => {
                        let dep_to = outcome
                            .targets
                            .get(via)
                            .cloned()
                            .unwrap_or_else(|| to.clone());
                        ChangeSource::PeerEscalation {
                            dependency: via.clone(),
                            to: dep_to,
                        }
                    }
                    _ => ChangeSource::Changeset {
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
                    },
                };
                ChangelogInput {
                    package: id.clone(),
                    from: from.clone(),
                    to: Some(to.clone()),
                    entries: vec![ChangelogEntry {
                        severity: sev,
                        source,
                    }],
                }
            };
            changelog_writes.push(crate::plan::ChangelogWrite {
                changelog_path: ch_path.clone(),
                input,
            });
        }
    }

    let mut diagnostics = outcome.diagnostics;
    diagnostics.extend(group_check.diagnostics);
    if agg.consumed.is_empty()
        && !opts.allow_empty_changesets
        && !ws.config.validation.allow_empty_changesets
    {
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

    Ok(VersionPlan {
        bumps,
        rewrites: outcome.rewrites.into_values().collect(),
        platform_writes: Vec::new(),
        optional_dep_updates: Vec::new(),
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

    fn git_init_with_commit(root: &Path) {
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "Test"],
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
        let ws =
            Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

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
        let ws =
            Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

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
        let ws =
            Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

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
        std::fs::write(
            root.join(".changeset/bump.md"),
            "---\n\"my-lib\": patch\n---\n\nfix.\n",
        )
        .unwrap();
        commit_all(root, "add package");

        let locator = IgnoreWalkLocator::new(root);
        let runner = NoopRunner;
        let ws =
            Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace must load");

        let inference = NoInference;
        let opts = VersionOptions {
            strict: false,
            strict_graph: false,
            allow_empty_changesets: true,
        };

        let plan = plan_version(&ws, &inference, &opts).expect("plan_version must succeed");

        assert!(
            plan.diagnostics.iter().any(|d| matches!(
                d.code,
                callisto_model::DiagnosticCode::NapiTargetAddedNotInMembers
            )),
            "VersionPlan.diagnostics must include the napi-drift diagnostic produced by pre_mutation_checks; got: {:?}",
            plan.diagnostics
        );
    }
}
