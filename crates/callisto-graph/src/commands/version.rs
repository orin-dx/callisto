use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};

use crate::changelog::{ChangeSource, ChangelogEntry, ChangelogInput};
use callisto_manifests::{open, OpenContext};
use callisto_model::{BumpReason, CommandRunner, GroupKind, ManifestRole, Severity};

use crate::aggregate::aggregate;
use crate::cascade::{run_cascade, CascadeInput};
use crate::commands::escalate;
use crate::commands::git_batch::{batch_blobs, BatchBlob};
use crate::config::groups::{GroupMember, GroupMemberKind};
use crate::error::GraphError;
use crate::infer::SeverityInference;
use crate::plan::{PlannedBump, PlatformWrite, VersionPlan, VersionWriteTarget};
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct VersionOptions {
    pub strict: bool,
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

    let pre_path = ws.root.join(ws.config.pre_json_path());
    let pre_text = callisto_model::strict_fs::read_optional(&pre_path)
        .map_err(|e| GraphError::PreJsonRead { message: e.to_string() })?;
    let (pre_state, pre_json_original_text) = if let Some(text) = pre_text {
        let state = callisto_model::format::parse_pre_json(&text).map_err(GraphError::PreJson)?;
        let state = ws.resolve_pre_state_from_json(state, &text)?;
        (Some(state), Some(text))
    } else {
        (None, None)
    };

    let mut agg = aggregate(
        &ws.graph,
        &ws.identity,
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
    if let Some(ref pre) = pre_state
        && pre.mode == callisto_model::format::PreMode::Exit
    {
        for (id, ver) in &base_versions {
            if ver.is_prerelease() {
                agg.severities.entry(id.clone()).or_insert(Severity::Patch);
                agg.reasons
                    .entry(id.clone())
                    .or_insert(BumpReason::PreRelease { tag: pre.tag.clone() });
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

    let napi = crate::napi::NapiTargetsIndex::load(&ws.config.groups, &ws.root, &ws.graph)?;
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
        let pkg = pkg_map
            .get(id)
            .copied()
            .expect("every bumped package is in the workspace");
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
            let mut input = match agg.changelog_inputs.get(id).cloned() {
                Some(mut agg_input) => {
                    // Real changeset data: use it, but set the resolved target version.
                    agg_input.from = from.clone();
                    agg_input.to = Some(to.clone());
                    if let Some(reason) = outcome.reasons.get(id)
                        && let Some(source) = map_reason_to_change_source(reason, id, &to, &outcome, &agg)
                    {
                        agg_input.entries.push(ChangelogEntry { severity: sev, source });
                    }
                    agg_input
                }
                _ => {
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

    let is_pre_mode = pre_state
        .as_ref()
        .map(|s| s.mode == callisto_model::format::PreMode::Pre)
        .unwrap_or(false);
    // In pre mode a changeset already recorded in pre.json is not pending: a rerun plans nothing for it.
    let nothing_pending = if is_pre_mode {
        agg.new_pre_changesets.is_empty()
    } else {
        agg.consumed.is_empty()
    };

    // Reading the changesets comes first: an entry that names no package leaves its changeset on disk.
    let mut diagnostics = agg.diagnostics.clone();
    diagnostics.extend(outcome.diagnostics);
    diagnostics.extend(group_check.diagnostics);
    if nothing_pending && !opts.allow_empty_changesets && !ws.config.validation.allow_empty_changesets {
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

    escalate(&mut diagnostics, opts.strict);

    let (pre_state_update, delete_pre_json) = if let Some(mut state) = pre_state {
        if state.mode == callisto_model::format::PreMode::Exit {
            (None, Some(ws.config.pre_json_path()))
        } else {
            // Records ids aggregate() determined are newly-seen this run; see Aggregation::new_pre_changesets.
            for id in &agg.new_pre_changesets {
                if !state.changesets.iter().any(|s| s == id) {
                    state.changesets.push(id.clone());
                }
            }
            (Some(state), None)
        }
    } else {
        (None, None)
    };

    let targets = bumps.iter().map(|b| (b.package.clone(), b.to.clone())).collect();
    let (platform_writes, optional_dep_updates) = platform_version_writes(ws, &targets)?;

    Ok(VersionPlan {
        bumps,
        rewrites: outcome.rewrites.into_values().collect(),
        platform_writes,
        optional_dep_updates,
        changelog_writes,
        consumed_changesets: agg.consumed,
        pre_state_update,
        pre_json_path: ws.config.pre_json_path(),
        pre_json_original_text,
        delete_pre_json,
        pre_cursor_updates: Vec::new(),
        observed_versions: base_versions,
        diagnostics,
    })
}

/// Refuses when a canonical manifest version differs from `HEAD` while release inputs remain or the change is staged.
pub fn check_partial_run<R: CommandRunner, D: DependencyResolver>(ws: &Workspace<'_, R, D>) -> Result<(), GraphError> {
    let head = match ws.git_access().head_sha() {
        Ok(head) => head,
        Err(error) => {
            let output = ws
                .runner
                .run("git", &["rev-parse", "--verify", "--quiet", "HEAD"], &ws.root)
                .map_err(GraphError::Command)?;
            match output.exit_code {
                Some(1) => return Ok(()),
                Some(0) => return Err(error.into()),
                exit_code => {
                    return Err(GraphError::Command(callisto_model::CommandError::Failed {
                        program: "git".to_string(),
                        exit_code,
                        stderr: output.stderr,
                    }))
                }
            }
        }
    };
    let head = head.as_str();

    // A pre-release cycle legitimately reruns `version` uncommitted; the staged-manifest check is scoped past it.
    let pre_mode = {
        let path = ws.root.join(ws.config.pre_json_path());
        let text = callisto_model::strict_fs::read_optional(&path)
            .map_err(|e| GraphError::PreJsonRead { message: e.to_string() })?;
        if let Some(text) = text {
            let state = callisto_model::format::parse_pre_json(&text).map_err(GraphError::PreJson)?;
            Some(state.mode)
        } else {
            None
        }
    };
    let in_pre_release_cycle = pre_mode == Some(callisto_model::format::PreMode::Pre);

    let changed = SourceChanges::since(ws, head)?;
    let staged_manifests = if in_pre_release_cycle {
        BTreeSet::new()
    } else {
        canonical_manifest_staged_paths(ws, head, &changed)?
    };
    let pending = !in_pre_release_cycle
        && (head_has_pending_changeset(ws, head)?
            || working_tree_has_pending_changeset(ws)?
            || !staged_manifests.is_empty());
    if !pending {
        return Ok(());
    }

    let head_texts = RefCell::new(HashMap::new());
    let index_texts = RefCell::new(HashMap::new());
    prefetch_head_manifests(ws, head, &head_texts)?;
    let mut head_reads = callisto_manifests::WorkspaceReads::default();
    let mut index_reads = callisto_manifests::WorkspaceReads::default();
    let base_versions = ws.base_versions()?;
    for pkg in ws.graph.packages() {
        let Some(disk) = base_versions.get(&pkg.id) else {
            continue;
        };
        for decl in pkg.canonical_manifests() {
            let path = super::release_decision::normalized_git_path(&decl.path)?;
            let Some(head_version) = manifest_version_at(ws, head, &path, &head_texts, &mut head_reads)? else {
                continue;
            };
            let index_path_changed = version_source_paths(ws, head, &changed, decl)?
                .iter()
                .any(|source| staged_manifests.contains(source));
            let index_version = if index_path_changed {
                manifest_version_in_index(ws, head, &path, &index_texts, &mut index_reads)?
            } else {
                None
            };
            let index_display = match &index_version {
                Some(version) => version.version.render().to_string(),
                None if index_path_changed => "absent".to_string(),
                None => head_version.version.render().to_string(),
            };
            let index_differs = index_path_changed
                && index_version
                    .as_ref()
                    .is_none_or(|version| version.version != head_version.version);
            let completing_prerelease = pre_mode == Some(callisto_model::format::PreMode::Exit)
                && disk.is_prerelease()
                && (!index_path_changed
                    || index_version
                        .as_ref()
                        .is_some_and(|version| version.version.is_prerelease()));
            if completing_prerelease {
                continue;
            }
            if *disk != head_version.version || index_differs {
                return Err(GraphError::PartialVersionRun {
                    path: head_version.authority,
                    disk: disk.clone(),
                    index: index_display,
                    head: head_version.version,
                });
            }
        }
    }

    Ok(())
}

/// Reads every canonical manifest at `head` in one `git cat-file --batch`, so the version reads that follow cost no
/// process each. A failed batch is not an error: those reads then run one at a time, as they always could.
fn prefetch_head_manifests<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    head: &str,
    texts: &ManifestTexts,
) -> Result<(), GraphError> {
    let mut paths = BTreeSet::new();
    for decl in ws.graph.packages().flat_map(|pkg| pkg.canonical_manifests()) {
        paths.insert(super::release_decision::normalized_git_path(&decl.path)?);
    }
    let objects: Vec<String> = paths.iter().map(|path| format!("{head}:{path}")).collect();
    let Ok(blobs) = batch_blobs(ws.runner, &ws.root, &objects) else {
        return Ok(());
    };
    let mut texts = texts.borrow_mut();
    for (path, blob) in paths.into_iter().zip(blobs) {
        texts.insert(
            path,
            match blob {
                BatchBlob::Blob(text) => Some(text),
                BatchBlob::Missing => None,
            },
        );
    }
    Ok(())
}

/// True when an untracked or tracked changeset remains in the configured directory on disk.
fn working_tree_has_pending_changeset<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
) -> Result<bool, GraphError> {
    let dir = ws.root.join(&ws.config.changesets_dir);
    let files = crate::aggregate::list_changeset_files(&dir).map_err(|error| GraphError::ApplyIo {
        path: ws.config.changesets_dir.clone(),
        message: error.to_string(),
    })?;
    Ok(!files.is_empty())
}

/// True when a canonical manifest differs between index and HEAD: `git add`ed by a prior `apply` but never committed.
fn canonical_manifest_staged_paths<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    head: &str,
    changed: &SourceChanges,
) -> Result<BTreeSet<String>, GraphError> {
    let mut paths = BTreeSet::new();
    for decl in ws.graph.packages().flat_map(|pkg| pkg.canonical_manifests()) {
        paths.extend(version_source_paths(ws, head, changed, decl)?);
    }
    // The index-versus-HEAD diff `SourceChanges` already read answers this for every path at once.
    Ok(paths.into_iter().filter(|path| changed.staged.contains(path)).collect())
}

/// Whether `<rev>:<path>` names an object; `--quiet` keeps a missing path from printing to stderr.
fn head_object_exists<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    spec: &str,
) -> Result<bool, GraphError> {
    let output = ws
        .runner
        .run("git", &["rev-parse", "--verify", "--quiet", spec], &ws.root)
        .map_err(GraphError::Command)?;
    match output.exit_code {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        exit_code => Err(GraphError::Command(callisto_model::CommandError::Failed {
            program: "git".to_string(),
            exit_code,
            stderr: output.stderr,
        })),
    }
}

/// True when `HEAD`'s tree still has an unconsumed changeset file, read from the commit, not the working tree.
fn head_has_pending_changeset<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    head: &str,
) -> Result<bool, GraphError> {
    let dir = super::release_decision::normalized_git_path(&ws.config.changesets_dir)?;
    let spec = format!("{head}:{dir}");
    if !head_object_exists(ws, &spec)? {
        // The changesets directory didn't exist at HEAD (e.g. the first `version` run); nothing left pending.
        return Ok(false);
    }
    let output = ws
        .runner
        .run("git", &["ls-tree", "--name-only", &spec], &ws.root)
        .map_err(GraphError::Command)?;
    if !output.success() {
        return Err(GraphError::Command(callisto_model::CommandError::Failed {
            program: "git".to_string(),
            exit_code: output.exit_code,
            stderr: output.stderr,
        }));
    }
    Ok(output.stdout.lines().any(|name| {
        let name = name.trim();
        name.ends_with(".md") && name != "README.md"
    }))
}

fn manifest_blob_at<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    head: &str,
    path: &str,
) -> Result<Option<String>, GraphError> {
    let spec = format!("{head}:{path}");
    if !head_object_exists(ws, &spec)? {
        return Ok(None);
    }
    let output = ws
        .runner
        .run("git", &["show", &spec], &ws.root)
        .map_err(GraphError::Command)?;
    if !output.success() {
        return Err(GraphError::Command(callisto_model::CommandError::Failed {
            program: "git".to_string(),
            exit_code: output.exit_code,
            stderr: output.stderr,
        }));
    }
    Ok(Some(output.stdout))
}

fn manifest_blob_in_index<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    path: &str,
) -> Result<Option<String>, GraphError> {
    let spec = format!(":{path}");
    if !head_object_exists(ws, &spec)? {
        return Ok(None);
    }
    let output = ws
        .runner
        .run("git", &["show", &spec], &ws.root)
        .map_err(GraphError::Command)?;
    if !output.success() {
        return Err(GraphError::Command(callisto_model::CommandError::Failed {
            program: "git".to_string(),
            exit_code: output.exit_code,
            stderr: output.stderr,
        }));
    }
    Ok(Some(output.stdout))
}

/// The paths that differ from `HEAD` in the working tree or the index, and those that differ in the index alone.
struct SourceChanges {
    changed: BTreeSet<String>,
    staged: BTreeSet<String>,
}

impl SourceChanges {
    fn since<R: CommandRunner, D: DependencyResolver>(
        ws: &Workspace<'_, R, D>,
        head: &str,
    ) -> Result<Self, GraphError> {
        let mut changed = BTreeSet::new();
        let mut staged = BTreeSet::new();
        let working_tree = vec!["diff", "--name-only", "-z", head];
        let index = vec!["diff", "--cached", "--name-only", "-z", head];
        for (args, is_index) in [(working_tree, false), (index, true)] {
            let output = ws.runner.run("git", &args, &ws.root).map_err(GraphError::Command)?;
            if !output.success() {
                return Err(GraphError::Command(callisto_model::CommandError::Failed {
                    program: "git".to_string(),
                    exit_code: output.exit_code,
                    stderr: output.stderr,
                }));
            }
            let paths = output
                .stdout
                .split('\0')
                .filter(|path| !path.is_empty())
                .map(str::to_owned);
            if is_index {
                let paths: Vec<String> = paths.collect();
                staged.extend(paths.iter().cloned());
                changed.extend(paths);
            } else {
                changed.extend(paths);
            }
        }
        Ok(SourceChanges { changed, staged })
    }
}

/// Every path whose staged change could alter the effective version of `decl`: its own manifest and, for Cargo, each
/// manifest that could be the workspace root it inherits from. That includes the workspace the package names in the
/// working tree, and, when its own manifest has changed since `HEAD`, the one it named at `HEAD` and in the index.
fn version_source_paths<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    head: &str,
    changed: &SourceChanges,
    decl: &callisto_model::ManifestDecl,
) -> Result<Vec<String>, GraphError> {
    let own = super::release_decision::normalized_git_path(&decl.path)?;
    let mut sources = vec![own.clone()];
    if decl.ecosystem() == callisto_model::Ecosystem::Cargo {
        for candidate in callisto_manifests::workspace_manifest_candidates(&ws.root, &decl.path)? {
            sources.push(super::release_decision::normalized_git_path(&candidate)?);
        }
        if changed.changed.contains(&own) {
            for text in [manifest_blob_at(ws, head, &own)?, manifest_blob_in_index(ws, &own)?]
                .into_iter()
                .flatten()
            {
                if let Some(named) = callisto_manifests::named_workspace_manifest(&decl.path, &text)? {
                    sources.push(super::release_decision::normalized_git_path(&named)?);
                }
            }
        }
    }
    Ok(sources)
}

/// Manifest text by normalized path, fetched at most once per path.
type ManifestTexts = RefCell<HashMap<String, Option<String>>>;

fn cached_text(
    texts: &ManifestTexts,
    path: &std::path::Path,
    fetch: impl FnOnce(&str) -> Result<Option<String>, GraphError>,
) -> Result<Option<String>, GraphError> {
    let key = super::release_decision::normalized_git_path(path)?;
    if let Some(text) = texts.borrow().get(&key) {
        return Ok(text.clone());
    }
    let text = fetch(&key)?;
    texts.borrow_mut().insert(key, text.clone());
    Ok(text)
}

fn manifest_version_at<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    head: &str,
    path: &str,
    texts: &ManifestTexts,
    reads: &mut callisto_manifests::WorkspaceReads,
) -> Result<Option<callisto_manifests::EffectiveManifestVersion>, GraphError> {
    let path = std::path::Path::new(path);
    let Ok(format) = callisto_model::ManifestFormat::from_path(path) else {
        return Ok(None);
    };
    let at_head = |other: &std::path::Path| cached_text(texts, other, |key| manifest_blob_at(ws, head, key));
    let Some(source) = at_head(path)? else {
        return Ok(None);
    };
    callisto_manifests::read_effective_version(format, &source, path, reads, at_head)
}

/// The version `path` has in the index; manifests it inherits from that the index lacks are read from `head`.
fn manifest_version_in_index<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    head: &str,
    path: &str,
    texts: &ManifestTexts,
    reads: &mut callisto_manifests::WorkspaceReads,
) -> Result<Option<callisto_manifests::EffectiveManifestVersion>, GraphError> {
    let manifest_path = std::path::Path::new(path);
    let Ok(format) = callisto_model::ManifestFormat::from_path(manifest_path) else {
        return Ok(None);
    };
    let Some(source) = manifest_blob_in_index(ws, path)? else {
        return Ok(None);
    };
    let in_index_or_head = |other: &std::path::Path| {
        cached_text(texts, other, |key| match manifest_blob_in_index(ws, key)? {
            Some(text) => Ok(Some(text)),
            None => manifest_blob_at(ws, head, key),
        })
    };
    callisto_manifests::read_effective_version(format, &source, manifest_path, reads, in_index_or_head)
}

/// Platform manifest version writes and owner `optionalDependencies` pin updates
/// for every owner with a target version in `targets`. Sources: each owner's
/// attached platform manifests, plus `[[fixed-group]]`
/// platform members.
pub(crate) fn platform_version_writes<R: CommandRunner, D: DependencyResolver>(
    ws: &Workspace<'_, R, D>,
    targets: &std::collections::BTreeMap<callisto_model::PackageId, callisto_model::Version>,
) -> Result<(Vec<PlatformWrite>, Vec<crate::plan::OptionalDepUpdate>), GraphError> {
    let open_ctx = OpenContext::for_workspace_root(&ws.root)?;
    let pkg_map: HashMap<_, _> = ws.graph.packages().map(|p| (&p.id, p)).collect();

    let mut members: Vec<(&callisto_model::PackageId, &std::path::Path, &str)> = Vec::new();
    for pkg in ws.graph.packages() {
        members.extend(
            ws.identity
                .attached_platforms(pkg)
                .map(|(name, path)| (&pkg.id, path, name)),
        );
    }
    for group in ws.config.groups.fixed.values() {
        for member in group.members(GroupMemberKind::PlatformManifest) {
            if let GroupMember::PlatformManifest { owner, path, name, .. } = member {
                members.push((owner, path, name));
            }
        }
    }

    let mut seen = HashSet::new();
    let mut platform_writes = Vec::new();
    let mut optional_dep_map: std::collections::BTreeMap<std::path::PathBuf, Vec<(String, callisto_model::Version)>> =
        std::collections::BTreeMap::new();
    for (owner, path, name) in members {
        let Some(to) = targets.get(owner) else {
            continue;
        };
        if !seen.insert(path) {
            continue;
        }
        let fmt = callisto_model::ManifestFormat::from_path(path)?;
        let decl = callisto_model::ManifestDecl::new(path, ManifestRole::Canonical, fmt)?;
        let handle = open(&decl, &open_ctx)?;
        let current = handle.current_version()?;
        platform_writes.push(PlatformWrite {
            manifest: path.to_path_buf(),
            version: to.clone(),
            from: current,
        });

        // A dual-manifest owner declares its platforms in package.json, not Cargo.toml.
        for owner_decl in pkg_map.get(owner).into_iter().flat_map(|p| p.canonical_manifests()) {
            let owner_fmt = callisto_model::ManifestFormat::from_path(&owner_decl.path)?;
            let owner_manifest_decl =
                callisto_model::ManifestDecl::new(owner_decl.path.clone(), ManifestRole::Canonical, owner_fmt)?;
            // Cached: owner manifest is shared across every platform target under it.
            let owner_handle = crate::manifest_cache::open_cached(&ws.manifest_cache, &owner_manifest_decl, &open_ctx)?;
            if owner_handle
                .iter_dependencies()
                .any(|dep| dep.kind == callisto_model::DepKind::Optional && dep.name == name)
            {
                optional_dep_map
                    .entry(owner_decl.path.clone())
                    .or_default()
                    .push((name.to_string(), to.clone()));
                break;
            }
        }
    }
    let optional_dep_updates = optional_dep_map
        .into_iter()
        .map(|(manifest, updates)| crate::plan::OptionalDepUpdate { manifest, updates })
        .collect();
    Ok((platform_writes, optional_dep_updates))
}

#[cfg(test)]
mod tests;
