use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::changelog::{ChangeSource, ChangelogEntry, ChangelogInput};
use callisto_model::format::{parse_changeset, Changeset};
use callisto_model::vcs::GitAccess;
use callisto_model::{
    BumpReason, CommitSha, Diagnostic, Ecosystem, Package, PackageId, ReleaseTrigger, Severity, Version,
};

use crate::config::resolve::resolve_package_config;
use crate::config::GroupTable;
use crate::config::{PreMajorInferencePolicy, ResolvedConfig};
use crate::error::GraphError;
use crate::identity::IdentityIndex;
use crate::infer::SeverityInference;
use crate::resolver::DependencyResolver;
use crate::tags::TagIndex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedChangeset {
    pub path: PathBuf,
    pub id: String,
    pub changeset: Changeset,
}

#[derive(Clone, Debug, Default)]
pub struct Aggregation {
    pub severities: BTreeMap<PackageId, Severity>,
    pub reasons: BTreeMap<PackageId, BumpReason>,
    pub named_by: BTreeMap<PackageId, NamedBy>,
    /// Changeset files to delete from disk this run (non-pre-mode only: a pre-mode run must never delete a
    /// changeset, since it may still be re-applied on the next pre-mode `version` before `pre exit`).
    pub consumed: Vec<PathBuf>,
    /// Changeset ids matched this run in pre mode that are not already in `pre.json`'s `changesets` list; the
    /// caller records these into `pre.json` so a rerun recognizes them as already-applied and skips re-adding
    /// their changelog entry, without deleting the file.
    pub new_pre_changesets: Vec<String>,
    /// True when at least one changeset (new or already recorded) matched a real package in pre mode this run.
    /// Distinct from `new_pre_changesets` being empty: a rerun with nothing new still has an active pre-release
    /// changeset driving severity, so the "no pending changesets" warning must key off this, not off "nothing new".
    pub pre_mode_has_active_changeset: bool,
    /// Packages whose severity this run came solely from a pre-mode changeset already recorded in `pre.json` --
    /// re-affirmed, not newly applied. The caller must not synthesize a changelog entry for these: it was already
    /// logged on the run that first recorded the changeset.
    pub pre_mode_recorded_only: std::collections::HashSet<PackageId>,
    pub changelog_inputs: BTreeMap<PackageId, ChangelogInput>,
    pub inference_commits: BTreeMap<PackageId, Vec<(CommitSha, String)>>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedBy {
    Changeset,
    Inference,
}

/// One `.changeset/*.md` file that failed `parse_changeset`, kept alongside its (workspace-root-
/// relative) path so a caller that must survive a malformed changeset -- `status` -- can report
/// it as a diagnostic instead of aborting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangesetLoadFailure {
    pub path: PathBuf,
    pub source: callisto_model::format::ParseError,
}

/// Every changeset `*.md` file in `dir` (not the README), sorted. A missing directory holds none; any other failure,
/// including an entry that cannot be read, is an error rather than a changeset left out of the release.
pub(crate) fn list_changeset_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let named =
        |path: &Path, error: std::io::Error| std::io::Error::new(error.kind(), format!("{}: {error}", path.display()));
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(named(dir, error)),
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry.map_err(|error| named(dir, error))?.path();
        let is_changeset = path.extension().and_then(|ext| ext.to_str()) == Some("md")
            && path.file_name().and_then(|name| name.to_str()) != Some("README.md");
        if !is_changeset {
            continue;
        }
        // A dangling link (an editor lock file such as `.#x.md`) or an entry removed mid-listing holds no changeset.
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => files.push(path),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(named(&path, error)),
        }
    }
    files.sort();
    Ok(files)
}

/// Loads and parses every `.changeset/*.md` file, splitting successes from parse failures
/// rather than aborting on the first one -- `status` must report every unparseable changeset,
/// not just the first. [`load_changesets`] collapses this to the first failure for callers
/// (`aggregate`/`version`) that must hard-fail instead.
pub fn load_changesets_permissive(
    root: &Path,
    cfg: &ResolvedConfig,
) -> Result<(Vec<LoadedChangeset>, Vec<ChangesetLoadFailure>), GraphError> {
    let dir = root.join(&cfg.changesets_dir);
    let files = list_changeset_files(&dir).map_err(|e| callisto_model::ManifestError::Read {
        path: dir.clone(),
        message: e.to_string(),
    })?;

    let mut loaded = Vec::new();
    let mut failures = Vec::new();
    for path in files {
        let content = fs::read_to_string(&path).map_err(|e| callisto_model::ManifestError::Read {
            path: path.clone(),
            message: e.to_string(),
        })?;
        let rel_path = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        match parse_changeset(&content) {
            Ok(changeset) => {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                loaded.push(LoadedChangeset {
                    path: rel_path,
                    id: stem,
                    changeset,
                });
            }
            Err(source) => failures.push(ChangesetLoadFailure { path: rel_path, source }),
        }
    }

    Ok((loaded, failures))
}

pub fn load_changesets(root: &Path, cfg: &ResolvedConfig) -> Result<Vec<LoadedChangeset>, GraphError> {
    let (loaded, mut failures) = load_changesets_permissive(root, cfg)?;
    if !failures.is_empty() {
        let failure = failures.remove(0);
        return Err(GraphError::ParseChangeset {
            path: failure.path,
            source: failure.source,
        });
    }
    Ok(loaded)
}

pub fn apply_pre_major(
    inferred: Severity,
    policy: PreMajorInferencePolicy,
    current: &Version,
    has_prior_release: bool,
) -> (Severity, bool) {
    if policy == PreMajorInferencePolicy::Off {
        return (inferred, false);
    }
    if current.major() != Some(0) || current.minor() == Some(0) || !has_prior_release {
        return (inferred, false);
    }

    match (policy, inferred) {
        (PreMajorInferencePolicy::Conservative | PreMajorInferencePolicy::ConservativeFeat, Severity::Major) => {
            (Severity::Minor, true)
        }
        (PreMajorInferencePolicy::ConservativeFeat, Severity::Minor) => (Severity::Patch, true),
        (_, s) => (s, false),
    }
}

/// Resolves a release tag name to the commit SHA it points at, so that
/// severity inference can be scoped to `since..HEAD` instead of walking the
/// entire history on every `aggregate()`-driven command.
///
/// Any failure to resolve the tag (missing, unborn repo, etc.) degrades to
/// `None`, which callers treat as "infer over full history".
fn resolve_since(git: &GitAccess<'_>, tag_name: &str) -> Option<CommitSha> {
    git.resolve_commit(tag_name).ok().flatten()
}

/// Resolves a changeset entry's or `--package` argument's parsed `PackageId`
/// against the packages in the graph, through [`IdentityIndex::resolve_id`] --
/// the one selector resolver, not a `PackageId::matches` scan (which treats a
/// Bare query as an ecosystem wildcard and would wrongly let a qualified
/// selector for one ecosystem select a same-named package in another).
///
/// `Ok(None)`: no match (unknown package, reported separately by
/// `validate`). `Ok(Some(pkg))`: unambiguous. `Err(AmbiguousName)`: two or
/// more matches (E103).
pub(crate) fn resolve_target_package<'a>(
    packages: impl Iterator<Item = &'a Package>,
    identity: &IdentityIndex,
    id: &PackageId,
) -> Result<Option<&'a Package>, GraphError> {
    match identity.resolve_id(id) {
        Ok(resolved) => Ok(packages.into_iter().find(|p| p.id == resolved)),
        Err(GraphError::UnknownPackage { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn aggregate<D, I>(
    graph: &D,
    identity: &IdentityIndex,
    config: &ResolvedConfig,
    git: &GitAccess<'_>,
    tags: &TagIndex,
    base_versions: &BTreeMap<PackageId, Version>,
    pre: Option<&callisto_model::format::PreState>,
    inference: &I,
) -> Result<Aggregation, GraphError>
where
    D: DependencyResolver,
    I: SeverityInference,
{
    let loaded = load_changesets(&config.root, config)?;
    let mut agg = Aggregation::default();

    for pkg in graph.packages() {
        let cur_sev = agg.severities.get(&pkg.id).copied().unwrap_or(Severity::None);
        let pathspecs: Vec<PathBuf> = crate::changed::package_paths(pkg);
        let last_tag = tags.last_tag(&pkg.id);
        let cur_ver = last_tag
            .map(|t| t.version.clone())
            .or_else(|| base_versions.get(&pkg.id).cloned())
            .ok_or_else(|| {
                GraphError::Manifest(callisto_model::ManifestError::MissingField {
                    path: pkg.manifests.first().map(|m| m.path.clone()).unwrap_or_default(),
                    field: "version",
                })
            })?;

        let package_ecosystems: Vec<Ecosystem> = pkg.canonical_manifests().map(|m| m.ecosystem()).collect();
        let policy = resolve_package_config(&pkg.id, &package_ecosystems, config)?
            .and_then(|pcfg| pcfg.pre_major_inference)
            .unwrap_or(PreMajorInferencePolicy::Off);

        // release_trigger: Changeset packages take severity from pending changesets only --
        // commit inference must not run for them, so neither does resolving its window.
        let inferred = if pkg.release_trigger == ReleaseTrigger::Auto {
            let window = crate::infer::InferenceWindowSpec {
                pathspecs: &pathspecs,
                since: last_tag.and_then(|t| resolve_since(git, t.name.as_str())),
                current_version: &cur_ver,
                has_prior_release: last_tag.is_some(),
                policy,
            };
            inference.infer(pkg, git, window)
        } else {
            Ok(None)
        };

        match inferred {
            Ok(Some(outcome)) => {
                if outcome.severity > cur_sev {
                    agg.severities.insert(pkg.id.clone(), outcome.severity);
                    agg.reasons.insert(
                        pkg.id.clone(),
                        BumpReason::Inference {
                            commits: outcome.commit_count,
                            remapped: outcome.remapped,
                        },
                    );
                    agg.named_by.insert(pkg.id.clone(), NamedBy::Inference);
                    agg.inference_commits.insert(pkg.id.clone(), outcome.commits.clone());
                }
            }
            Ok(None) => {}
            Err(e) => {
                agg.diagnostics.push(Diagnostic {
                    code: callisto_model::DiagnosticCode::PreMajorInferenceInert,
                    severity: callisto_model::DiagnosticSeverity::Warning,
                    message: format!("Commit inference failed for package `{}`: {e}", pkg.id.display_name()),
                    package: Some(pkg.id.clone()),
                    path: None,
                    governed_by: None,
                    escalated_by: None,
                });
            }
        }
    }

    // During a pre-release cycle (PreMode::Pre) changesets must NOT be consumed:
    // they remain on disk so they can be re-applied when the cycle exits.
    let is_pre_mode = pre
        .map(|s| s.mode == callisto_model::format::PreMode::Pre)
        .unwrap_or(false);
    // Ids already recorded in pre.json still resolve for severity, but are not re-added to the changelog.
    let already_recorded: std::collections::HashSet<&str> = pre
        .map(|s| s.changesets.iter().map(String::as_str).collect())
        .unwrap_or_default();

    for cs in loaded {
        let is_already_recorded = is_pre_mode && already_recorded.contains(cs.id.as_str());
        // Defer adding to `consumed` until after we confirm at least one entry
        // resolved to a real workspace package. A changeset where every entry
        // names a removed package must NOT be consumed (which would delete it
        // on disk); instead, an UnknownPackage diagnostic is emitted and the
        // file is left for the user to clean up manually.
        //
        // A changeset with a MIX of resolved and unresolved entries must also
        // stay unconsumed: consuming it would delete the file (or, in pre
        // mode, mark it as already-applied) and permanently lose the
        // unresolved entry's bump once that entry's target reappears.
        let mut matched_any = false;
        let mut has_unresolved = false;
        for entry in cs.changeset.entries {
            let id = match PackageId::parse(&entry.name) {
                Ok(id) => id,
                Err(_) => {
                    has_unresolved = true;
                    agg.diagnostics.push(Diagnostic {
                        code: callisto_model::DiagnosticCode::UnknownPackage,
                        severity: callisto_model::DiagnosticSeverity::Warning,
                        message: format!(
                            "Changeset `{}` contains invalid package name `{}`; the changeset \
                             will not be consumed until this entry is resolved",
                            cs.path.display(),
                            entry.name
                        ),
                        package: None,
                        path: Some(cs.path.clone()),
                        governed_by: None,
                        escalated_by: None,
                    });
                    continue;
                }
            };
            match resolve_target_package(graph.packages(), identity, &id)? {
                Some(target_pkg) => {
                    matched_any = true;
                    let canonical_id = target_pkg.id.clone();
                    let cur_sev = agg.severities.get(&canonical_id).copied().unwrap_or(Severity::None);
                    if entry.severity > cur_sev {
                        agg.severities.insert(canonical_id.clone(), entry.severity);
                        agg.reasons.insert(
                            canonical_id.clone(),
                            BumpReason::Changeset {
                                changesets: vec![cs.id.clone()],
                            },
                        );
                        agg.named_by.insert(canonical_id.clone(), NamedBy::Changeset);
                    }

                    if entry.severity != Severity::None && is_already_recorded {
                        agg.pre_mode_recorded_only.insert(canonical_id.clone());
                    }
                    if entry.severity != Severity::None && !is_already_recorded {
                        // In pre-release mode use the pre-cycle entry version as the
                        // changelog "from" baseline so the log covers the full pre
                        // range rather than reflecting live (pre-tagged) versions.
                        let pkg_ver = if is_pre_mode {
                            pre.and_then(|s| crate::pre_initial_version(s, &canonical_id))
                                .cloned()
                                .or_else(|| base_versions.get(&canonical_id).cloned())
                                .unwrap_or_else(|| Version::semver(0, 0, 0))
                        } else {
                            tags.last_tag(&canonical_id)
                                .map(|t| t.version.clone())
                                .or_else(|| base_versions.get(&canonical_id).cloned())
                                .unwrap_or_else(|| Version::semver(0, 0, 0))
                        };
                        let cl_input =
                            agg.changelog_inputs
                                .entry(canonical_id.clone())
                                .or_insert_with(|| ChangelogInput {
                                    package: canonical_id.clone(),
                                    from: pkg_ver,
                                    to: None,
                                    entries: Vec::new(),
                                });
                        cl_input.entries.push(ChangelogEntry {
                            severity: entry.severity,
                            source: ChangeSource::Changeset {
                                filename: cs.id.clone(),
                                summary: cs.changeset.summary.clone(),
                            },
                        });
                    }
                }
                None => {
                    // Entry references a package not in the workspace (e.g. a
                    // package that was removed since the changeset was written).
                    // Emit a diagnostic so the user knows, but do NOT count
                    // this as a match -- a fully-orphaned changeset stays on
                    // disk rather than being silently deleted.
                    has_unresolved = true;
                    agg.diagnostics.push(Diagnostic {
                        code: callisto_model::DiagnosticCode::UnknownPackage,
                        severity: callisto_model::DiagnosticSeverity::Warning,
                        message: format!(
                            "Changeset `{}` references package `{}` which is not in the \
                             workspace; the changeset will not be consumed until this entry \
                             is resolved",
                            cs.path.display(),
                            entry.name
                        ),
                        package: None,
                        path: Some(cs.path.clone()),
                        governed_by: None,
                        escalated_by: None,
                    });
                }
            }
        }
        // A changeset with any unresolved entry (orphaned or invalid) is left on disk and
        // unrecorded regardless of mode, so a later rerun can still pick up its bump.
        if matched_any && !has_unresolved {
            if is_pre_mode {
                agg.pre_mode_has_active_changeset = true;
                // Records newly-seen ids so a rerun treats them as already-applied and skips the changelog entry.
                if !is_already_recorded {
                    agg.new_pre_changesets.push(cs.id.clone());
                }
            } else {
                agg.consumed.push(cs.path.clone());
            }
        }
    }

    loop {
        let mut changed = false;
        if union_fixed(&mut agg, &config.groups, base_versions) {
            changed = true;
        }
        if union_linked(&mut agg, &config.groups, base_versions) {
            changed = true;
        }
        if !changed {
            break;
        }
    }

    Ok(agg)
}

pub(crate) fn union_fixed(
    agg: &mut Aggregation,
    groups: &GroupTable,
    base_versions: &BTreeMap<PackageId, Version>,
) -> bool {
    let mut changed = false;
    for g in groups.fixed.values() {
        let target = g.max_severity(&agg.severities);
        if target == Severity::None {
            continue;
        }

        for m in g.package_members().cloned().collect::<Vec<_>>() {
            let cur = agg.severities.get(&m).copied().unwrap_or(Severity::None);
            if target > cur {
                // Guard against stale group members: a package listed in the
                // config group that was subsequently removed from the workspace
                // must not be inserted into severities.  Doing so causes
                // `bump_target` in `solve_cascade` to call
                // `input.base.get(stale_id)` -> `None` ->
                // `Err(GraphError::Manifest(MissingField))`, which surfaces as
                // a misleading crash.  Emit a warning instead and skip.
                if !base_versions.contains_key(&m) {
                    agg.diagnostics.push(Diagnostic {
                        code: callisto_model::DiagnosticCode::UnknownPackage,
                        severity: callisto_model::DiagnosticSeverity::Warning,
                        message: format!(
                            "Fixed group `{}` references package `{}` which is not in the \
                             workspace; the stale group member is skipped. Remove it from \
                             callisto.toml to silence this warning.",
                            g.name,
                            m.display_name()
                        ),
                        package: Some(m.clone()),
                        path: None,
                        governed_by: Some(callisto_model::ConfigKey::FIXED_GROUP),
                        escalated_by: None,
                    });
                    continue;
                }
                agg.severities.insert(m.clone(), target);
                agg.reasons
                    .insert(m.clone(), BumpReason::FixedGroupUnion { group: g.name.clone() });
                changed = true;
            }
        }
    }
    changed
}

pub(crate) fn union_linked(
    agg: &mut Aggregation,
    groups: &GroupTable,
    base_versions: &BTreeMap<PackageId, Version>,
) -> bool {
    let mut changed = false;
    for g in groups.linked.values() {
        let named_any = g.package_members().any(|id| agg.named_by.contains_key(id));
        if !named_any {
            continue;
        }

        // Unlike `union_fixed`'s max_severity (over every member), a linked
        // group's target severity comes only from members a changeset or
        // inference actually named -- an unnamed sibling's stale leftover
        // severity from an earlier fixed-point round must not count.
        let target_sev = g
            .package_members()
            .filter(|id| agg.named_by.contains_key(*id))
            .filter_map(|id| agg.severities.get(id).copied())
            .max()
            .unwrap_or(Severity::None);

        for m in g.package_members().cloned().collect::<Vec<_>>() {
            let cur = agg.severities.get(&m).copied().unwrap_or(Severity::None);
            if target_sev > cur {
                // Guard against stale linked-group members, same rationale as
                // in `union_fixed`: a removed package must not enter
                // `agg.severities`, which would cause `bump_target` to crash.
                if !base_versions.contains_key(&m) {
                    agg.diagnostics.push(Diagnostic {
                        code: callisto_model::DiagnosticCode::UnknownPackage,
                        severity: callisto_model::DiagnosticSeverity::Warning,
                        message: format!(
                            "Linked group `{}` references package `{}` which is not in the \
                             workspace; the stale group member is skipped. Remove it from \
                             callisto.toml to silence this warning.",
                            g.name,
                            m.display_name()
                        ),
                        package: Some(m.clone()),
                        path: None,
                        governed_by: Some(callisto_model::ConfigKey::LINKED_GROUP),
                        escalated_by: None,
                    });
                    continue;
                }
                agg.severities.insert(m.clone(), target_sev);
                agg.reasons
                    .insert(m.clone(), BumpReason::LinkedGroupUnion { group: g.name.clone() });
                changed = true;
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests;
