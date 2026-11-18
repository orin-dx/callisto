use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::changelog::{ChangeSource, ChangelogEntry, ChangelogInput};
use callisto_model::format::{parse_changeset, Changeset};
use callisto_model::vcs::GitAccess;
use callisto_model::{
    BumpReason, CommitSha, Diagnostic, GroupKind, GroupName, Package, PackageId, ReleaseTrigger, Severity, Version,
};

use crate::config::resolve::resolve_package_config;
use crate::config::GroupTable;
use crate::config::{PreMajorInferencePolicy, ResolvedConfig};
use crate::error::{ConfigError, GraphError};
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
    /// Every package with releasable input before cascade; each holds at least one fresh source.
    pub seeds: BTreeMap<PackageId, Seed>,
    /// Changeset files to delete from disk this run (non-pre-mode only: a pre-mode run must never delete a
    /// changeset, since it may still be re-applied on the next pre-mode `version` before `pre exit`).
    pub consumed: Vec<PathBuf>,
    /// Changeset ids matched this run in pre mode that are not already in `pre.json`'s `changesets` list; the
    /// caller records these into `pre.json` so a rerun recognizes them as already-applied and skips re-adding
    /// their changelog entry, without deleting the file.
    pub new_pre_changesets: Vec<String>,
    /// Changeset files a Changesets v3 pre cycle moves into `.changeset/pre/` this run, marking them applied.
    pub archived: Vec<crate::plan::ArchivedChangeset>,
    pub changelog_inputs: BTreeMap<PackageId, ChangelogInput>,
    pub inference_commits: BTreeMap<PackageId, Vec<(CommitSha, String)>>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Aggregation {
    /// Each seeded package's severity, the cascade's starting point.
    pub fn severities(&self) -> BTreeMap<PackageId, Severity> {
        self.seeds
            .iter()
            .map(|(id, seed)| (id.clone(), seed.severity()))
            .collect()
    }

    /// Each seeded package's reason.
    pub fn reasons(&self) -> BTreeMap<PackageId, BumpReason> {
        self.seeds
            .iter()
            .map(|(id, seed)| (id.clone(), seed.reason()))
            .collect()
    }

    fn add(&mut self, id: &PackageId, severity: Severity, source: Source) {
        match self.seeds.get_mut(id) {
            Some(seed) => seed.add(severity, source),
            None => {
                self.seeds.insert(id.clone(), Seed::new(severity, source));
            }
        }
    }

    fn severity(&self, id: &PackageId) -> Severity {
        self.seeds.get(id).map(Seed::severity).unwrap_or(Severity::None)
    }
}

/// What put a package into the release before cascade.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A changeset this run applies for the first time.
    FreshChangeset {
        id: String,
    },
    /// A changeset an earlier run of the current pre cycle applied: it raises severity but releases nothing alone.
    RetainedChangeset {
        id: String,
    },
    Inference {
        commits: usize,
        remapped: bool,
    },
    /// A fixed or linked group union, added only when a member has fresh input.
    Group {
        group: GroupName,
        kind: GroupKind,
    },
}

impl Source {
    pub fn is_fresh(&self) -> bool {
        match self {
            Source::FreshChangeset { .. } | Source::Inference { .. } | Source::Group { .. } => true,
            Source::RetainedChangeset { .. } => false,
        }
    }

    /// Named directly by a changeset or inference rather than pulled in by a group.
    fn is_named(&self) -> bool {
        match self {
            Source::FreshChangeset { .. } | Source::RetainedChangeset { .. } | Source::Inference { .. } => true,
            Source::Group { .. } => false,
        }
    }

    fn reason(&self) -> BumpReason {
        match self {
            Source::FreshChangeset { id } | Source::RetainedChangeset { id } => BumpReason::Changeset {
                changesets: vec![id.clone()],
            },
            Source::Inference { commits, remapped } => BumpReason::Inference {
                commits: *commits,
                remapped: *remapped,
            },
            Source::Group {
                group,
                kind: GroupKind::Fixed,
            } => BumpReason::FixedGroupUnion { group: group.clone() },
            Source::Group {
                group,
                kind: GroupKind::Linked,
            } => BumpReason::LinkedGroupUnion { group: group.clone() },
        }
    }
}

/// One package's sources, each with the severity it asks for; never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seed {
    first: (Severity, Source),
    rest: Vec<(Severity, Source)>,
}

impl Seed {
    fn new(severity: Severity, source: Source) -> Self {
        Seed {
            first: (severity, source),
            rest: Vec::new(),
        }
    }

    fn add(&mut self, severity: Severity, source: Source) {
        self.rest.push((severity, source));
    }

    fn entries(&self) -> impl Iterator<Item = &(Severity, Source)> {
        std::iter::once(&self.first).chain(&self.rest)
    }

    pub fn sources(&self) -> impl Iterator<Item = &Source> {
        self.entries().map(|(_, source)| source)
    }

    /// The highest severity any source asks for, retained ones included.
    pub fn severity(&self) -> Severity {
        self.entries()
            .map(|(severity, _)| *severity)
            .fold(self.first.0, Severity::max)
    }

    pub fn is_fresh(&self) -> bool {
        self.sources().any(Source::is_fresh)
    }

    fn is_named(&self) -> bool {
        self.sources().any(Source::is_named)
    }

    /// The reason of the first fresh source at the highest fresh severity; the first source when none is fresh.
    pub fn reason(&self) -> BumpReason {
        let mut best = &self.first;
        for entry in self.entries().filter(|(_, source)| source.is_fresh()) {
            if !best.1.is_fresh() || entry.0 > best.0 {
                best = entry;
            }
        }
        best.1.reason()
    }
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
    load_changesets_in(root, &cfg.changesets_dir)
}

fn load_changesets_in(
    root: &Path,
    dir: &Path,
) -> Result<(Vec<LoadedChangeset>, Vec<ChangesetLoadFailure>), GraphError> {
    let dir = root.join(dir);
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
                for entry in &changeset.entries {
                    ConfigError::reject_colon_form(&entry.name, &format!("changeset `{}`", rel_path.display()))?;
                }
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
    strict(load_changesets_permissive(root, cfg)?)
}

/// The changesets a Changesets v3 pre cycle archived in `.changeset/pre/`, which earlier prereleases already applied.
/// Prefixes the id of a changeset loaded from a v3 pre cycle's archive.
pub(crate) const ARCHIVED_ID_PREFIX: &str = "pre/";

/// [`load_archived_changesets`] with each unparseable archived note returned beside the parsed ones.
pub(crate) fn load_archived_changesets_permissive(
    root: &Path,
    cfg: &ResolvedConfig,
) -> Result<(Vec<LoadedChangeset>, Vec<ChangesetLoadFailure>), GraphError> {
    load_changesets_in(root, &cfg.pre_archive_dir())
}

pub fn load_archived_changesets(root: &Path, cfg: &ResolvedConfig) -> Result<Vec<LoadedChangeset>, GraphError> {
    let mut archived = strict(load_changesets_in(root, &cfg.pre_archive_dir())?)?;
    // A new note may reuse an archived note's name; the two are distinct changes.
    for cs in &mut archived {
        cs.id = format!("{ARCHIVED_ID_PREFIX}{}", cs.id);
    }
    Ok(archived)
}

fn strict(
    (loaded, mut failures): (Vec<LoadedChangeset>, Vec<ChangesetLoadFailure>),
) -> Result<Vec<LoadedChangeset>, GraphError> {
    if !failures.is_empty() {
        let failure = failures.remove(0);
        return Err(GraphError::ParseChangeset {
            path: failure.path,
            source: failure.source,
        });
    }
    Ok(loaded)
}

/// The first of `<stem>.md`, `<stem>-2.md`, ... in the archive that is free or already holds `note`, and whether it does.
fn archive_slot(
    archive_dir: &Path,
    from: &Path,
    note: &Changeset,
    taken: &BTreeMap<std::ffi::OsString, Changeset>,
) -> (PathBuf, bool) {
    let stem = from.file_stem().and_then(|stem| stem.to_str()).unwrap_or("changeset");
    let mut name = format!("{stem}.md");
    let mut n = 2;
    loop {
        match taken.get(std::ffi::OsStr::new(&name)) {
            None => return (archive_dir.join(name), false),
            Some(archived) if archived == note => return (archive_dir.join(name), true),
            Some(_) => {
                name = format!("{stem}-{n}.md");
                n += 1;
            }
        }
    }
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
/// A tag that is listed but does not resolve to a commit is an error: guessing "infer over full history" would
/// plan a bump from commits the last release already covered.
fn resolve_since(git: &GitAccess<'_>, tags: &TagIndex, tag_name: &str) -> Result<CommitSha, GraphError> {
    tags.commit_of(git, tag_name)?.ok_or_else(|| {
        GraphError::Vcs(callisto_model::vcs::VcsError::RefNotFound {
            ref_name: tag_name.to_string(),
        })
    })
}

/// Resolves a changeset entry's or `--package` argument's parsed `PackageId`
/// against the packages in the graph, through [`IdentityIndex::resolve_id`] --
/// the one selector resolver, not a `PackageId::matches` scan (which treats a
/// Bare query as an ecosystem wildcard and would wrongly let a qualified
/// selector for one ecosystem select a same-named package in another).
///
/// `Ok(None)`: no match (unknown package, reported separately by
/// `status`). `Ok(Some(pkg))`: unambiguous. `Err(AmbiguousName)`: two or
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
    let mut loaded = load_changesets(&config.root, config)?;
    // A v3 cycle archives applied changesets in `pre/`: retained while the cycle runs, consumed with the rest at exit.
    let archived = match pre.map(callisto_model::format::PreState::shape) {
        Some(callisto_model::format::PreStateShape::V3) => load_archived_changesets(&config.root, config)?,
        Some(callisto_model::format::PreStateShape::V2) | None => Vec::new(),
    };
    let archived_paths: std::collections::HashSet<PathBuf> = archived.iter().map(|cs| cs.path.clone()).collect();
    let mut archive_names: BTreeMap<std::ffi::OsString, Changeset> = archived
        .iter()
        .filter_map(|cs| Some((cs.path.file_name()?.to_os_string(), cs.changeset.clone())))
        .collect();
    let archive_dir = config.pre_archive_dir();
    let is_v3 = pre.map(callisto_model::format::PreState::shape) == Some(callisto_model::format::PreStateShape::V3);
    loaded.extend(archived);
    let mut agg = Aggregation::default();
    // Inference reads `since..HEAD`; in a shallow clone that range is silently shorter than the real one.
    if graph.packages().any(|pkg| pkg.release_trigger == ReleaseTrigger::Auto) {
        git.require_full_history()?;
    }

    for pkg in graph.packages() {
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

        let native_names: Vec<_> = identity.native_names(&pkg.id).collect();
        let policy = resolve_package_config(&native_names, config)
            .and_then(|pcfg| pcfg.pre_major_inference)
            .unwrap_or(PreMajorInferencePolicy::Off);

        // release_trigger: Changeset packages take severity from pending changesets only --
        // commit inference must not run for them, so neither does resolving its window.
        let inferred = if pkg.release_trigger == ReleaseTrigger::Auto {
            let since = last_tag
                .map(|t| resolve_since(git, tags, t.name.as_str()))
                .transpose()?;
            let window = crate::infer::InferenceWindowSpec {
                pathspecs: &pathspecs,
                since,
                current_version: &cur_ver,
                has_prior_release: last_tag.is_some(),
                policy,
            };
            inference.infer(pkg, git, window)?
        } else {
            None
        };

        if let Some(outcome) = inferred
            && outcome.severity > Severity::None
        {
            agg.add(
                &pkg.id,
                outcome.severity,
                Source::Inference {
                    commits: outcome.commit_count,
                    remapped: outcome.remapped,
                },
            );
            agg.inference_commits.insert(pkg.id.clone(), outcome.commits.clone());
        }
    }

    // During a pre-release cycle (PreMode::Pre) changesets must NOT be consumed:
    // they remain on disk so they can be re-applied when the cycle exits.
    let is_pre_mode = pre
        .map(|s| s.mode == callisto_model::format::PreMode::Pre)
        .unwrap_or(false);
    let is_exiting = pre.is_some_and(|s| s.mode == callisto_model::format::PreMode::Exit);
    // Ids already recorded in pre.json still resolve for severity, but are not re-added to the changelog.
    let already_recorded: std::collections::HashSet<&str> = pre
        .and_then(|s| s.changesets())
        .map(|ids| ids.iter().map(String::as_str).collect())
        .unwrap_or_default();

    for cs in loaded {
        // Where this note goes in the archive; already there means it duplicates a note this cycle released.
        let slot = (is_pre_mode && is_v3 && !archived_paths.contains(&cs.path))
            .then(|| archive_slot(&archive_dir, &cs.path, &cs.changeset, &archive_names));
        let unfinished_move = slot.as_ref().is_some_and(|(_, present)| *present);
        let is_already_recorded = is_pre_mode
            && (already_recorded.contains(cs.id.as_str()) || archived_paths.contains(&cs.path) || unfinished_move);
        if let Some((to, true)) = &slot {
            agg.archived.push(crate::plan::ArchivedChangeset {
                from: cs.path.clone(),
                to: to.clone(),
                duplicate: true,
            });
        }
        // Consume a changeset only once an entry resolves; a mixed or fully unresolved one stays on disk and is ignored whole.
        let mut has_unresolved = false;
        let mut resolved = Vec::new();
        let note = cs.changeset.clone();
        for entry in cs.changeset.entries {
            let unresolved = match PackageId::parse(&entry.name) {
                Err(_) => Some("contains invalid package name"),
                Ok(id) => match resolve_target_package(graph.packages(), identity, &id)? {
                    Some(target_pkg) => {
                        resolved.push((entry, target_pkg));
                        continue;
                    }
                    // Exit deletes pre.json, so an archived note skipped here would never be released.
                    None if is_exiting && archived_paths.contains(&cs.path) => {
                        return Err(GraphError::UnknownPackage {
                            id,
                            hint: format!(
                                "\nThe archived changeset `{}` names it; remove that entry or restore the package, then run `version` again.",
                                cs.path.display()
                            ),
                        });
                    }
                    None => Some("references a package that is not in the workspace:"),
                },
            };
            if let Some(what) = unresolved {
                has_unresolved = true;
                agg.diagnostics.push(Diagnostic {
                    code: callisto_model::DiagnosticCode::UnknownPackage,
                    severity: callisto_model::DiagnosticSeverity::Warning,
                    message: format!(
                        "Changeset `{}` {what} `{}`; none of its entries apply until this entry is fixed or removed",
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
        if has_unresolved {
            continue;
        }
        let mut matched_any = false;
        for (entry, target_pkg) in resolved {
            matched_any = true;
            let canonical_id = target_pkg.id.clone();
            if entry.severity == Severity::None {
                continue;
            }
            let id = cs.id.clone();
            let source = if is_already_recorded {
                Source::RetainedChangeset { id }
            } else {
                Source::FreshChangeset { id }
            };
            agg.add(&canonical_id, entry.severity, source);
            if !is_already_recorded {
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
                let cl_input = agg
                    .changelog_inputs
                    .entry(canonical_id.clone())
                    .or_insert_with(|| ChangelogInput {
                        style: Default::default(),
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
        if matched_any {
            if is_pre_mode {
                // Records newly-seen ids so a rerun treats them as already-applied and skips the changelog entry.
                if !is_already_recorded {
                    agg.new_pre_changesets.push(cs.id.clone());
                    if let Some((to, false)) = slot {
                        if let Some(name) = to.file_name() {
                            archive_names.insert(name.to_os_string(), note.clone());
                        }
                        agg.archived.push(crate::plan::ArchivedChangeset {
                            from: cs.path.clone(),
                            to,
                            duplicate: false,
                        });
                    }
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
    // Retained changesets alone release nothing: a rerun of a pre cycle plans no further prerelease for them.
    agg.seeds.retain(|_, seed| seed.is_fresh());

    Ok(agg)
}

pub(crate) fn union_fixed(
    agg: &mut Aggregation,
    groups: &GroupTable,
    base_versions: &BTreeMap<PackageId, Version>,
) -> bool {
    let mut changed = false;
    for g in groups.fixed.values() {
        let target = g
            .package_members()
            .filter_map(|id| agg.seeds.get(id))
            .map(Seed::severity)
            .fold(Severity::None, Severity::max);
        changed |= union_group(agg, g, target, base_versions);
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
        // A linked group's target comes only from members a changeset or inference named, not stale sibling severities.
        let target = g
            .package_members()
            .filter_map(|id| agg.seeds.get(id))
            .filter(|seed| seed.is_named())
            .map(Seed::severity)
            .fold(Severity::None, Severity::max);
        changed |= union_group(agg, g, target, base_versions);
    }
    changed
}

/// Raises every live member of `g` to `target` when a member has fresh input; a group whose members hold only
/// retained changesets releases nothing.
fn union_group(
    agg: &mut Aggregation,
    g: &crate::config::GroupDef,
    target: Severity,
    base_versions: &BTreeMap<PackageId, Version>,
) -> bool {
    let fresh = g
        .package_members()
        .any(|id| agg.seeds.get(id).is_some_and(Seed::is_fresh));
    if target == Severity::None || !fresh {
        return false;
    }
    let mut changed = false;
    for m in g.package_members().cloned().collect::<Vec<_>>() {
        let member_fresh = agg.seeds.get(&m).is_some_and(Seed::is_fresh);
        if target <= agg.severity(&m) && member_fresh {
            continue;
        }
        // Skip group members absent from the workspace: inserting one would make `bump_target` fail on a missing base version.
        if !base_versions.contains_key(&m) {
            let (label, key) = match g.kind {
                GroupKind::Fixed => ("Fixed", callisto_model::ConfigKey::FIXED_GROUP),
                GroupKind::Linked => ("Linked", callisto_model::ConfigKey::LINKED_GROUP),
            };
            agg.diagnostics.push(Diagnostic {
                code: callisto_model::DiagnosticCode::UnknownPackage,
                severity: callisto_model::DiagnosticSeverity::Warning,
                message: format!(
                    "{label} group `{}` references package `{}` which is not in the \
                     workspace; the stale group member is skipped. Remove it from \
                     callisto.toml to silence this warning.",
                    g.name,
                    m.display_name()
                ),
                package: Some(m.clone()),
                path: None,
                governed_by: Some(key),
                escalated_by: None,
            });
            continue;
        }
        agg.add(
            &m,
            target,
            Source::Group {
                group: g.name.clone(),
                kind: g.kind,
            },
        );
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests;
