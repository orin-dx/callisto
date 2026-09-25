use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use callisto_changelog::{ChangeSource, ChangelogEntry, ChangelogInput};
use callisto_format::{parse_changeset, Changeset};
use callisto_model::{BumpReason, CommitSha, Diagnostic, Package, PackageId, ReleaseTrigger, Severity, Version};
use callisto_vcs::GitAccess;

use crate::config::resolve::resolve_package_config;
use crate::config::GroupTable;
use crate::config::{PreMajorInferencePolicy, ResolvedConfig};
use crate::error::GraphError;
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
    /// Changeset files to delete from disk this run (non-pre-mode only: a
    /// pre-mode run must never delete a changeset, since it may still be
    /// re-applied on the next pre-mode `version` before `pre exit`).
    pub consumed: Vec<PathBuf>,
    /// Changeset ids matched this run in pre mode that are NOT already in
    /// `pre.json`'s `changesets` list -- the caller records these into
    /// `pre.json` so a rerun recognizes them as already-applied and skips
    /// re-adding their changelog entry, without deleting the file.
    pub new_pre_changesets: Vec<String>,
    /// True when at least one changeset (new or already recorded) matched a
    /// real package in pre mode this run. Distinct from `new_pre_changesets`
    /// being empty: a rerun with nothing NEW still has an active pre-release
    /// changeset driving its severity, so the "no pending changesets"
    /// warning must key off this, not off "nothing new".
    pub pre_mode_has_active_changeset: bool,
    pub changelog_inputs: BTreeMap<PackageId, ChangelogInput>,
    pub inference_commits: BTreeMap<PackageId, Vec<(CommitSha, String)>>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedBy {
    Changeset,
    Inference,
}

pub fn load_changesets(root: &Path, cfg: &ResolvedConfig) -> Result<Vec<LoadedChangeset>, GraphError> {
    let dir = root.join(&cfg.changesets_dir);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(&dir).map_err(|e| callisto_model::ManifestError::Read {
        path: dir.clone(),
        message: e.to_string(),
    })?;

    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("md") {
            if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                if file_name != "README.md" && file_name != "config.json" && file_name != "pre.json" {
                    files.push(path);
                }
            }
        }
    }

    files.sort();

    let mut loaded = Vec::new();
    for path in files {
        let content = fs::read_to_string(&path).map_err(|e| callisto_model::ManifestError::Read {
            path: path.clone(),
            message: e.to_string(),
        })?;
        let changeset = parse_changeset(&content).map_err(|e| GraphError::ParseChangeset {
            path: path.clone(),
            source: e,
        })?;
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let rel_path = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        loaded.push(LoadedChangeset {
            path: rel_path,
            id: stem,
            changeset,
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

/// Resolves a changeset entry's parsed `PackageId` against the packages in
/// the graph.
///
/// `PackageId::matches` is pairwise: a bare id and a prefixed id with the
/// same name are compatible, since bare doesn't specify an ecosystem. But
/// a polyglot workspace can legitimately have the same name in two-plus
/// ecosystems (`cargo/foo`, `npm/foo`), and a bare `foo` can't resolve to
/// either without more context. `.find()` over such a graph would silently
/// pick whichever candidate comes first -- the ambiguity bug this function
/// fixes by collecting *all* matches and only succeeding when there's
/// exactly one.
///
/// `Ok(None)`: no match (unknown package, reported separately by
/// `validate`). `Ok(Some(pkg))`: unambiguous. `Err(AmbiguousName)`: two or
/// more matches.
pub(crate) fn resolve_target_package<'a>(
    packages: impl Iterator<Item = &'a Package>,
    id: &PackageId,
) -> Result<Option<&'a Package>, GraphError> {
    id.resolve_unique(packages, |p| &p.id)
        .map_err(|candidates| GraphError::AmbiguousName {
            name: id.display_name(),
            candidates: candidates.iter().map(|p| p.id.clone()).collect(),
        })
}

pub fn aggregate<D, I>(
    graph: &D,
    config: &ResolvedConfig,
    git: &GitAccess<'_>,
    tags: &TagIndex,
    base_versions: &BTreeMap<PackageId, Version>,
    pre: Option<&callisto_format::PreState>,
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

        let policy = resolve_package_config(&pkg.id, config)?
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
    let is_pre_mode = pre.map(|s| s.mode == callisto_format::PreMode::Pre).unwrap_or(false);
    // Ids pre.json already recorded from an earlier pre-mode run: still
    // resolved for severity (so cascade sees the right target), but not
    // re-added to the changelog (already logged there).
    let already_recorded: std::collections::HashSet<&str> = pre
        .map(|s| s.changesets.iter().map(String::as_str).collect())
        .unwrap_or_default();

    for cs in loaded {
        let is_already_recorded = is_pre_mode && already_recorded.contains(cs.id.as_str());
        // Defer adding to `consumed` until after we confirm at least one entry
        // resolved to a real workspace package.  A changeset where every entry
        // names a removed package must NOT be consumed (which would delete it
        // on disk); instead, an UnknownPackage diagnostic is emitted and the
        // file is left for the user to clean up manually.
        let mut matched_any = false;
        for entry in cs.changeset.entries {
            let id = match PackageId::parse(&entry.name) {
                Ok(id) => id,
                Err(_) => {
                    agg.diagnostics.push(Diagnostic {
                        code: callisto_model::DiagnosticCode::UnknownPackage,
                        severity: callisto_model::DiagnosticSeverity::Warning,
                        message: format!(
                            "Changeset `{}` contains invalid package name `{}`",
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
            match resolve_target_package(graph.packages(), &id)? {
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

                    if entry.severity != Severity::None && !is_already_recorded {
                        // In pre-release mode use the pre-cycle entry version as the
                        // changelog "from" baseline so the log covers the full pre
                        // range rather than reflecting live (pre-tagged) versions.
                        let pkg_ver = if is_pre_mode {
                            pre.and_then(|s| s.initial_versions.get(crate::pre_json_key(&canonical_id)))
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
        // A fully-orphaned changeset (no entry resolved) is left on disk and
        // unrecorded regardless of mode.
        if matched_any {
            if is_pre_mode {
                agg.pre_mode_has_active_changeset = true;
                // Record newly-seen ids so a rerun treats them as
                // already-applied instead of re-adding their changelog
                // entry every time; the file itself stays on disk so it can
                // still be re-applied by a later pre-mode run before `pre
                // exit`, and its severity keeps counting against
                // `initialVersions` on every run via `already_recorded`.
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
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use callisto_model::{
        CommandError, CommandOutput, CommandRunner, DepEdge, GroupKind, GroupName, ManifestDecl, ManifestFormat,
        ManifestRole, Package,
    };

    use crate::config::{GroupDef, GroupMember};
    use crate::infer::{InferenceOutcome, InferenceWindowSpec, SeverityInference};
    use callisto_fixtures::git::{init_repo, run_git};

    /// Direct unit coverage for `apply_pre_major` across all three policy
    /// states, previously only exercised indirectly through full-config
    /// integration tests. `Off` never downgrades; `Conservative` downgrades
    /// Major->Minor only; `ConservativeFeat` downgrades both Major->Minor
    /// and Minor->Patch.
    #[test]
    fn apply_pre_major_off_never_downgrades() {
        let v = Version::semver(0, 1, 0);
        assert_eq!(
            apply_pre_major(Severity::Major, PreMajorInferencePolicy::Off, &v, true),
            (Severity::Major, false)
        );
        assert_eq!(
            apply_pre_major(Severity::Minor, PreMajorInferencePolicy::Off, &v, true),
            (Severity::Minor, false)
        );
    }

    #[test]
    fn apply_pre_major_conservative_downgrades_major_to_minor_only() {
        let v = Version::semver(0, 1, 0);
        assert_eq!(
            apply_pre_major(Severity::Major, PreMajorInferencePolicy::Conservative, &v, true),
            (Severity::Minor, true)
        );
        assert_eq!(
            apply_pre_major(Severity::Minor, PreMajorInferencePolicy::Conservative, &v, true),
            (Severity::Minor, false),
            "Conservative must not also downgrade Minor->Patch"
        );
    }

    #[test]
    fn apply_pre_major_conservative_feat_downgrades_both_levels() {
        let v = Version::semver(0, 1, 0);
        assert_eq!(
            apply_pre_major(Severity::Major, PreMajorInferencePolicy::ConservativeFeat, &v, true),
            (Severity::Minor, true)
        );
        assert_eq!(
            apply_pre_major(Severity::Minor, PreMajorInferencePolicy::ConservativeFeat, &v, true),
            (Severity::Patch, true)
        );
    }

    /// Shells out to the real `git` binary.
    struct RealGitRunner;

    impl CommandRunner for RealGitRunner {
        fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
            let output = std::process::Command::new(program)
                .args(args)
                .current_dir(cwd)
                .output()
                .map_err(|e| CommandError::Io {
                    program: program.to_string(),
                    message: e.to_string(),
                })?;
            Ok(CommandOutput {
                exit_code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
    }

    /// A `CommandRunner` double that answers `git rev-parse --verify --quiet
    /// <tag>^{commit}` with a canned SHA and counts invocations.
    struct FakeRevParseRunner {
        calls: AtomicUsize,
        tag: String,
        sha: CommitSha,
    }

    impl CommandRunner for FakeRevParseRunner {
        fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
            assert_eq!(program, "git");
            assert_eq!(
                args,
                [
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    format!("{}^{{commit}}", self.tag).as_str()
                ]
            );
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: format!("{}\n", self.sha.as_str()),
                stderr: String::new(),
            })
        }
    }

    /// Spec: `load_changesets` must include the filename in its error when a changeset file
    /// fails `parse_changeset`. The bare `?` propagation previously produced a
    /// `GraphError::Format(ParseError)` with no path context, making it impossible for a
    /// developer to triage which file caused the failure in a workspace with many changesets.
    #[test]
    fn test_load_changesets_error_includes_filename() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        let cs_dir = root.join(".changeset");
        std::fs::create_dir_all(&cs_dir).unwrap();

        // Missing `---` frontmatter delimiter — parse_changeset returns
        // ParseError::MissingFrontmatterStart. The error must carry the filename so the
        // developer can find the broken file.
        std::fs::write(cs_dir.join("malformed-changeset.md"), "cargo/foo: patch\n\nSummary.\n").unwrap();

        let cfg = crate::config::load(root).unwrap();
        let result = load_changesets(root, &cfg);

        let err = result.expect_err("load_changesets must return Err for a malformed changeset file");
        let err_display = format!("{err}");
        assert!(
            err_display.contains("malformed-changeset"),
            "error message must contain the offending filename so the developer can triage; \
             got: {err_display:?}"
        );
    }

    /// Spec: `resolve_since` resolves the tag with one `git rev-parse
    /// --verify --quiet <tag>^{commit}` rather than degrading to `None` (an
    /// unbounded full-history walk).
    #[test]
    fn test_resolve_since_resolves_the_tag_through_git() {
        let dir = tempfile::tempdir().unwrap();
        let sha = CommitSha::parse("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef").unwrap();
        let runner = FakeRevParseRunner {
            calls: AtomicUsize::new(0),
            tag: "pkg-a@1.0.0".to_string(),
            sha: sha.clone(),
        };
        let git = GitAccess::new(dir.path(), &runner);

        let resolved = resolve_since(&git, "pkg-a@1.0.0");

        assert_eq!(
            resolved,
            Some(sha),
            "resolve_since must resolve the tag, not silently return None"
        );
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    }

    struct SinglePackageGraph {
        pkg: Package,
    }

    impl DependencyResolver for SinglePackageGraph {
        fn packages(&self) -> impl Iterator<Item = &Package> {
            std::iter::once(&self.pkg)
        }

        fn dependencies_of(&self, _id: &PackageId) -> impl Iterator<Item = &DepEdge> {
            std::iter::empty()
        }

        fn dependents_of(&self, _id: &PackageId) -> impl Iterator<Item = &DepEdge> {
            std::iter::empty()
        }
    }

    /// Records the `since` value passed into `InferenceWindowSpec` without
    /// doing any real inference work.
    #[derive(Default)]
    struct RecordingInference {
        captured_since: Mutex<Option<CommitSha>>,
    }

    impl SeverityInference for RecordingInference {
        fn infer(
            &self,
            _pkg: &Package,
            _git: &GitAccess<'_>,
            window: InferenceWindowSpec<'_>,
        ) -> Result<Option<InferenceOutcome>, GraphError> {
            *self.captured_since.lock().unwrap() = window.since.clone();
            Ok(None)
        }
    }

    /// Spec: `aggregate()` must scope commit inference to `last_tag..HEAD`
    /// instead of walking full history on every run. Reproduces the bug by
    /// building a real one-package repo with a real release tag, then
    /// asserting the `since` field handed to `SeverityInference::infer`
    /// carries the commit SHA the tag points at (not `None`, which forces a
    /// full-history walk in `callisto_conventional::window::fetch_commits`).
    #[test]
    fn test_aggregate_scopes_inference_window_to_last_tag() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();

        init_repo(root);
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        let pkg_id = PackageId::parse("pkg-a").unwrap();
        let tag_name = format!("{}@1.0.0", pkg_id.display_name());
        // Explicit message + disabled gpg signing so this is robust
        // regardless of the developer machine's global git config (e.g.
        // `tag.forceSignAnnotated` / `tag.gpgSign`).
        run_git(root, &["-c", "tag.gpgSign=false", "tag", "-m", "release", &tag_name]);

        // A commit landing after the tag; a correctly-scoped inference
        // window must never need to look past `tag_name` to find it, but a
        // `since: None` (full history) window would happily walk right over
        // it and beyond, all the way back to the repo root.
        std::fs::write(root.join("CHANGES.md"), "more\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: add changes file"]);

        let expected_sha_output = std::process::Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", &format!("{tag_name}^{{commit}}")])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(expected_sha_output.status.success());
        let expected_sha = CommitSha::parse(String::from_utf8_lossy(&expected_sha_output.stdout).trim()).unwrap();

        let runner = RealGitRunner;
        let manifest = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
        let graph = SinglePackageGraph {
            pkg: Package {
                id: pkg_id.clone(),
                manifests: vec![manifest],
                changelog: None,
                release_trigger: callisto_model::ReleaseTrigger::Auto,
                publish_to: Vec::new(),
                tag_template: None,
            },
        };
        let git = GitAccess::new(root, &runner);
        let cfg = crate::config::load(root).unwrap();
        let tags = TagIndex::build(&git, &graph, &cfg).unwrap();

        // Sanity: the tag we just created was actually picked up.
        assert_eq!(
            tags.last_tag(&pkg_id).map(|t| t.version.render().to_string()),
            Some("1.0.0".to_string())
        );

        let inference = RecordingInference::default();
        let base_versions = BTreeMap::new();

        aggregate(&graph, &cfg, &git, &tags, &base_versions, None, &inference).unwrap();

        let captured = inference.captured_since.lock().unwrap().clone();
        assert_eq!(
            captured,
            Some(expected_sha),
            "aggregate() must scope inference to last_tag..HEAD instead of hardcoding `since: None` \
             (full history)"
        );
    }

    /// Records the `pathspecs` value passed into `InferenceWindowSpec`
    /// without doing any real inference work.
    #[derive(Default)]
    struct RecordingPathspecsInference {
        captured_pathspecs: Mutex<Vec<PathBuf>>,
    }

    impl SeverityInference for RecordingPathspecsInference {
        fn infer(
            &self,
            _pkg: &Package,
            _git: &GitAccess<'_>,
            window: InferenceWindowSpec<'_>,
        ) -> Result<Option<InferenceOutcome>, GraphError> {
            *self.captured_pathspecs.lock().unwrap() = window.pathspecs.to_vec();
            Ok(None)
        }
    }

    /// Spec: `aggregate()` must scope commit inference to the package's
    /// source *directory*, not the manifest *file* path -- otherwise
    /// inference only ever matches commits touching the manifest itself,
    /// never real source changes, making it a near-total no-op. Reproduces
    /// the bug directly: a package whose canonical manifest lives at
    /// `crates/pkg-a/Cargo.toml` must produce a pathspec of `crates/pkg-a`
    /// (mirroring `changed::package_paths`'s directory-scoping convention),
    /// not `crates/pkg-a/Cargo.toml`.
    #[test]
    fn test_aggregate_scopes_inference_pathspecs_to_package_directory() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        let pkg_id = PackageId::parse("pkg-a").unwrap();
        let manifest = ManifestDecl::new(
            "crates/pkg-a/Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        let graph = SinglePackageGraph {
            pkg: Package {
                id: pkg_id.clone(),
                manifests: vec![manifest],
                changelog: None,
                release_trigger: callisto_model::ReleaseTrigger::Auto,
                publish_to: Vec::new(),
                tag_template: None,
            },
        };

        let runner = RealGitRunner;
        let git = GitAccess::new(root, &runner);
        let cfg = crate::config::load(root).unwrap();
        let tags = TagIndex::build(&git, &graph, &cfg).unwrap();

        let inference = RecordingPathspecsInference::default();
        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_id, Version::semver(0, 1, 0));

        aggregate(&graph, &cfg, &git, &tags, &base_versions, None, &inference).unwrap();

        let captured = inference.captured_pathspecs.lock().unwrap().clone();
        assert_eq!(
            captured,
            vec![PathBuf::from("crates/pkg-a")],
            "aggregate() must scope inference pathspecs to the package's source directory, not the \
             manifest file path"
        );
    }

    struct FixedCommitsInference {
        commits: Vec<(CommitSha, String)>,
    }

    impl SeverityInference for FixedCommitsInference {
        fn infer(
            &self,
            _pkg: &Package,
            _git: &GitAccess<'_>,
            _window: InferenceWindowSpec<'_>,
        ) -> Result<Option<InferenceOutcome>, GraphError> {
            Ok(Some(InferenceOutcome {
                severity: Severity::Minor,
                commit_count: self.commits.len(),
                remapped: false,
                commits: self.commits.clone(),
            }))
        }
    }

    /// aggregate() must retain InferenceOutcome.commits
    /// on Aggregation.inference_commits keyed by package, not discard it
    /// after constructing BumpReason::Inference (which only carries a count).
    #[test]
    fn test_aggregate_retains_inference_commits_on_aggregation() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        let pkg_id = PackageId::parse("pkg-a").unwrap();
        let manifest = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
        let graph = SinglePackageGraph {
            pkg: Package {
                id: pkg_id.clone(),
                manifests: vec![manifest],
                changelog: None,
                release_trigger: callisto_model::ReleaseTrigger::Auto,
                publish_to: Vec::new(),
                tag_template: None,
            },
        };
        let runner = RealGitRunner;
        let git = GitAccess::new(root, &runner);
        let cfg = crate::config::load(root).unwrap();
        let tags = TagIndex::build(&git, &graph, &cfg).unwrap();

        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_id.clone(), callisto_model::Version::semver(1, 0, 0));

        let sha_recent = CommitSha::parse(&"a".repeat(40)).unwrap();
        let inference = FixedCommitsInference {
            commits: vec![(sha_recent.clone(), "feat: recent".to_string())],
        };

        let agg = aggregate(&graph, &cfg, &git, &tags, &base_versions, None, &inference).unwrap();

        assert_eq!(
            agg.inference_commits.get(&pkg_id),
            Some(&vec![(sha_recent, "feat: recent".to_string())]),
            "Aggregation.inference_commits must retain InferenceOutcome.commits for the package"
        );
    }

    /// Records how many times `infer()` was called, without doing any real
    /// inference work.
    #[derive(Default)]
    struct CountingInference {
        calls: AtomicUsize,
    }

    impl SeverityInference for CountingInference {
        fn infer(
            &self,
            _pkg: &Package,
            _git: &GitAccess<'_>,
            _window: InferenceWindowSpec<'_>,
        ) -> Result<Option<InferenceOutcome>, GraphError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    }

    /// A package resolved to `ReleaseTrigger::Changeset` (the default) must not
    /// have commit-based severity inference invoked at all, even when the `inference`
    /// feature is compiled in -- its severity comes only from pending changesets.
    #[test]
    fn test_aggregate_skips_inference_for_changeset_trigger() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        let pkg_id = PackageId::parse("pkg-a").unwrap();
        let manifest = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
        let graph = SinglePackageGraph {
            pkg: Package {
                id: pkg_id.clone(),
                manifests: vec![manifest],
                changelog: None,
                release_trigger: callisto_model::ReleaseTrigger::Changeset,
                publish_to: Vec::new(),
                tag_template: None,
            },
        };
        let runner = RealGitRunner;
        let git = GitAccess::new(root, &runner);
        let cfg = crate::config::load(root).unwrap();
        let tags = TagIndex::build(&git, &graph, &cfg).unwrap();

        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_id, callisto_model::Version::semver(1, 0, 0));

        let inference = CountingInference::default();
        aggregate(&graph, &cfg, &git, &tags, &base_versions, None, &inference).unwrap();

        assert_eq!(
            inference.calls.load(Ordering::SeqCst),
            0,
            "aggregate() must not invoke SeverityInference::infer for a package whose \
             resolved release_trigger is ReleaseTrigger::Changeset"
        );
    }

    /// A package resolved to `ReleaseTrigger::Auto` must still run commit-based
    /// severity inference, and its outcome can still raise the package's severity above
    /// what pending changesets alone would produce -- unchanged from current behavior.
    #[test]
    fn test_aggregate_runs_inference_for_auto_trigger_and_raises_severity() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        let pkg_id = PackageId::parse("pkg-a").unwrap();
        let manifest = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
        let graph = SinglePackageGraph {
            pkg: Package {
                id: pkg_id.clone(),
                manifests: vec![manifest],
                changelog: None,
                release_trigger: callisto_model::ReleaseTrigger::Auto,
                publish_to: Vec::new(),
                tag_template: None,
            },
        };
        let runner = RealGitRunner;
        let git = GitAccess::new(root, &runner);
        let cfg = crate::config::load(root).unwrap();
        let tags = TagIndex::build(&git, &graph, &cfg).unwrap();

        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_id.clone(), callisto_model::Version::semver(1, 0, 0));

        // No changesets on disk: with no other signal, severity stays `None`
        // unless inference itself raises it.
        let inference = FixedCommitsInference {
            commits: vec![(CommitSha::parse(&"b".repeat(40)).unwrap(), "feat: auto".to_string())],
        };

        let agg = aggregate(&graph, &cfg, &git, &tags, &base_versions, None, &inference).unwrap();

        assert_eq!(
            agg.severities.get(&pkg_id),
            Some(&Severity::Minor),
            "aggregate() must invoke inference for ReleaseTrigger::Auto and let its outcome \
             raise the package's severity"
        );
    }

    /// Spec: `aggregate()` scopes inference to the commit the last tag
    /// points at.
    #[test]
    fn test_aggregate_resolves_since_to_the_tag_commit() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();

        init_repo(root);
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        let pkg_id = PackageId::parse("pkg-a").unwrap();
        let tag_name = format!("{}@1.0.0", pkg_id.display_name());
        run_git(root, &["-c", "tag.gpgSign=false", "tag", "-m", "release", &tag_name]);

        std::fs::write(root.join("CHANGES.md"), "more\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: add changes file"]);

        let expected_sha_output = std::process::Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", &format!("{tag_name}^{{commit}}")])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(expected_sha_output.status.success());
        let expected_sha = CommitSha::parse(String::from_utf8_lossy(&expected_sha_output.stdout).trim()).unwrap();

        let runner = RealGitRunner;
        let manifest = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
        let graph = SinglePackageGraph {
            pkg: Package {
                id: pkg_id.clone(),
                manifests: vec![manifest],
                changelog: None,
                release_trigger: callisto_model::ReleaseTrigger::Auto,
                publish_to: Vec::new(),
                tag_template: None,
            },
        };
        let git = GitAccess::new(root, &runner);
        let cfg = crate::config::load(root).unwrap();
        let tags = TagIndex::build(&git, &graph, &cfg).unwrap();

        assert_eq!(
            tags.last_tag(&pkg_id).map(|t| t.version.render().to_string()),
            Some("1.0.0".to_string())
        );

        let inference = RecordingInference::default();
        let base_versions = BTreeMap::new();

        aggregate(&graph, &cfg, &git, &tags, &base_versions, None, &inference).unwrap();

        let captured = inference.captured_since.lock().unwrap().clone();
        assert_eq!(
            captured,
            Some(expected_sha),
            "aggregate() must resolve `since` to the last tag's commit"
        );
    }

    // Auto so aggregate() invokes inference for callers of this fixture that
    // exercise SeverityInference; trigger itself is irrelevant to the rest.
    fn make_pkg(id: PackageId) -> Package {
        let manifest = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
        Package {
            id,
            manifests: vec![manifest],
            changelog: None,
            release_trigger: callisto_model::ReleaseTrigger::Auto,
            publish_to: Vec::new(),
            tag_template: None,
        }
    }

    /// Spec: a changeset entry naming a package by its bare name (no
    /// ecosystem prefix) must NOT silently resolve against an arbitrary
    /// candidate when the graph contains packages in two or more ecosystems
    /// sharing that name. Resolving `foo` against both `cargo/foo` and
    /// `npm/foo` is genuinely ambiguous and must be a caller-visible error,
    /// not a first-match-wins pick based on iteration order.
    #[test]
    fn test_resolve_target_package_ambiguous_bare_name_errors() {
        let pkg_cargo = make_pkg(PackageId::parse("cargo/foo").unwrap());
        let pkg_npm = make_pkg(PackageId::parse("npm/foo").unwrap());
        let packages = [pkg_cargo, pkg_npm];
        let bare = PackageId::parse("foo").unwrap();

        let result = resolve_target_package(packages.iter(), &bare);

        match result {
            Err(GraphError::AmbiguousName { name, candidates }) => {
                assert_eq!(name, "foo");
                assert_eq!(candidates.len(), 2);
                assert!(candidates.contains(&PackageId::parse("cargo/foo").unwrap()));
                assert!(candidates.contains(&PackageId::parse("npm/foo").unwrap()));
            }
            other => panic!("expected GraphError::AmbiguousName, got {other:?}"),
        }
    }

    /// Spec: a bare-name lookup must still resolve fine when the name is
    /// unambiguous (only one package with that name across all ecosystems
    /// in the graph).
    #[test]
    fn test_resolve_target_package_unambiguous_bare_name_resolves() {
        let pkg_cargo = make_pkg(PackageId::parse("cargo/foo").unwrap());
        let pkg_other = make_pkg(PackageId::parse("cargo/bar").unwrap());
        let packages = [pkg_cargo, pkg_other];
        let bare = PackageId::parse("foo").unwrap();

        let result = resolve_target_package(packages.iter(), &bare).unwrap();

        assert_eq!(
            result.map(|p| p.id.clone()),
            Some(PackageId::parse("cargo/foo").unwrap())
        );
    }

    /// Spec: a bare-name lookup for a name that doesn't exist anywhere in
    /// the graph resolves to `None` (not an error) -- unknown-package
    /// reporting is the caller's responsibility (see validate.rs).
    #[test]
    fn test_resolve_target_package_unknown_name_returns_none() {
        let pkg_cargo = make_pkg(PackageId::parse("cargo/foo").unwrap());
        let packages = [pkg_cargo];
        let bare = PackageId::parse("does-not-exist").unwrap();

        let result = resolve_target_package(packages.iter(), &bare).unwrap();

        assert!(result.is_none());
    }

    /// Spec: the ambiguity check must not assume exactly two colliding
    /// candidates. A workspace with the same bare name registered in three
    /// or more ecosystems (cargo/foo, npm/foo, pypi/foo) must still report
    /// every candidate in `AmbiguousName`, not just the first two (an
    /// off-by-one truncation or a hardcoded pairwise assumption would not
    /// be caught by the two-ecosystem test above).
    #[test]
    fn test_resolve_target_package_ambiguous_bare_name_three_ecosystems_errors() {
        let pkg_cargo = make_pkg(PackageId::parse("cargo/foo").unwrap());
        let pkg_npm = make_pkg(PackageId::parse("npm/foo").unwrap());
        let pkg_pypi = make_pkg(PackageId::parse("pypi/foo").unwrap());
        let packages = [pkg_cargo, pkg_npm, pkg_pypi];
        let bare = PackageId::parse("foo").unwrap();

        let result = resolve_target_package(packages.iter(), &bare);

        match result {
            Err(GraphError::AmbiguousName { name, candidates }) => {
                assert_eq!(name, "foo");
                assert_eq!(candidates.len(), 3);
                assert!(candidates.contains(&PackageId::parse("cargo/foo").unwrap()));
                assert!(candidates.contains(&PackageId::parse("npm/foo").unwrap()));
                assert!(candidates.contains(&PackageId::parse("pypi/foo").unwrap()));
            }
            other => panic!("expected GraphError::AmbiguousName with 3 candidates, got {other:?}"),
        }
    }

    /// Spec: bare-name matching against `PackageId::name()` is a plain
    /// string comparison, which is case-sensitive. A package registered as
    /// `cargo/Foo` must NOT be resolved by a bare lookup for `foo` -- they
    /// are treated as distinct names, so the lookup resolves to `None`
    /// (unknown-package) rather than matching or erroring as ambiguous.
    /// This test pins down that actual behavior explicitly so a future
    /// change to case handling is a deliberate, visible decision.
    #[test]
    fn test_resolve_target_package_bare_name_matching_is_case_sensitive() {
        let pkg_cargo = make_pkg(PackageId::parse("cargo/Foo").unwrap());
        let packages = [pkg_cargo];
        let bare = PackageId::parse("foo").unwrap();

        let result = resolve_target_package(packages.iter(), &bare).unwrap();

        assert!(
            result.is_none(),
            "case-sensitive name comparison must not match 'foo' against 'Foo'"
        );
    }

    /// Spec: a changeset where EVERY entry references a package not in the
    /// workspace must NOT be added to `consumed` (which would silently delete
    /// it on disk) and must emit a `DiagnosticCode::UnknownPackage` warning
    /// for each orphaned entry.  On the current (unfixed) code, the changeset
    /// IS added to `consumed` before the entry loop, so it ends up deleted
    /// despite no version bump ever being recorded.
    #[test]
    fn test_orphaned_changeset_not_consumed_emits_unknown_package_diagnostic() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();

        // Minimal git repo so TagIndex::build can enumerate tags.
        init_repo(root);
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        // Changeset referencing only pkg-foo which is NOT in the workspace.
        let cs_dir = root.join(".changeset");
        std::fs::create_dir_all(&cs_dir).unwrap();
        std::fs::write(
            cs_dir.join("orphan-cs.md"),
            "---\n\"pkg-foo\": minor\n---\n\nOrphaned changeset.\n",
        )
        .unwrap();

        // Workspace has only pkg-bar.
        let pkg_bar_id = PackageId::parse("pkg-bar").unwrap();
        let graph = SinglePackageGraph {
            pkg: make_pkg(pkg_bar_id.clone()),
        };
        let cfg = crate::config::load(root).unwrap();
        let runner = RealGitRunner;
        let git = GitAccess::new(root, &runner);
        let tags = crate::tags::TagIndex::build(&git, &graph, &cfg).unwrap();

        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_bar_id.clone(), Version::semver(1, 0, 0));

        let inference = RecordingInference::default();
        let agg = aggregate(&graph, &cfg, &git, &tags, &base_versions, None, &inference).unwrap();

        assert!(
            agg.consumed.is_empty(),
            "a fully-orphaned changeset (all entries reference non-existent packages) must NOT \
             be added to consumed (which would cause it to be deleted on disk): got {:?}",
            agg.consumed
        );

        let unknown_pkg_diags: Vec<_> = agg
            .diagnostics
            .iter()
            .filter(|d| d.code == callisto_model::DiagnosticCode::UnknownPackage)
            .collect();
        assert!(
            !unknown_pkg_diags.is_empty(),
            "must emit at least one UnknownPackage diagnostic for orphaned changeset entries; \
             got diagnostics: {:?}",
            agg.diagnostics
        );
    }

    /// Spec: when a fixed group in callisto.toml references a package that
    /// no longer exists in the workspace, `union_fixed` must NOT insert
    /// the stale member into `agg.severities`. Doing so causes
    /// `bump_target` in `solve_cascade` to call
    /// `input.base.get(stale_id)` -> `None` ->
    /// `Err(GraphError::Manifest(MissingField))`, crashing `callisto
    /// version` with a misleading error. The stale member must be skipped
    /// and an `UnknownPackage` warning emitted.
    ///
    /// Setup: `pkg_bar` has `Severity::Minor` (from a changeset), `pkg_baz`
    /// has none yet. Fixed group has all three: `pkg_foo` (stale),
    /// `pkg_bar`, `pkg_baz`. `union_fixed` should propagate `Minor` to
    /// `pkg_baz`, skip `pkg_foo` with a diagnostic, and
    /// return `true` because `pkg_baz` changed.
    #[test]
    fn test_union_fixed_stale_member_emits_diagnostic_and_is_skipped() {
        let pkg_foo = PackageId::parse("pkg-foo").unwrap(); // stale: removed from workspace
        let pkg_bar = PackageId::parse("pkg-bar").unwrap(); // real workspace package (has severity)
        let pkg_baz = PackageId::parse("pkg-baz").unwrap(); // real workspace package (no severity yet)

        let mut agg = Aggregation::default();
        // pkg-bar has a changeset-driven Minor bump; pkg-baz has nothing yet.
        agg.severities.insert(pkg_bar.clone(), Severity::Minor);
        agg.named_by.insert(pkg_bar.clone(), NamedBy::Changeset);

        let mut groups = GroupTable::default();
        let group_def = GroupDef {
            name: GroupName("fixed-grp".to_string()),
            kind: GroupKind::Fixed,
            members: vec![
                GroupMember::Package(pkg_foo.clone()),
                GroupMember::Package(pkg_bar.clone()),
                GroupMember::Package(pkg_baz.clone()),
            ],
        };
        groups.fixed.insert(group_def.name.clone(), group_def);

        // Only pkg-bar and pkg-baz are in the workspace; pkg-foo is stale.
        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_bar.clone(), Version::semver(1, 0, 0));
        base_versions.insert(pkg_baz.clone(), Version::semver(1, 0, 0));

        let changed = union_fixed(&mut agg, &groups, &base_versions);

        // pkg_baz had no severity but should now have Minor propagated from pkg_bar.
        assert!(
            changed,
            "union_fixed must return true because pkg-baz received a propagated severity"
        );
        assert_eq!(
            agg.severities.get(&pkg_baz),
            Some(&Severity::Minor),
            "real member pkg-baz must receive the propagated Minor severity"
        );
        // The stale member must never enter severities.
        assert!(
            !agg.severities.contains_key(&pkg_foo),
            "stale group member pkg-foo must NOT be inserted into severities (would crash cascade \
             with a misleading MissingField error)"
        );

        let unknown_diags: Vec<_> = agg
            .diagnostics
            .iter()
            .filter(|d| d.code == callisto_model::DiagnosticCode::UnknownPackage)
            .collect();
        assert!(
            !unknown_diags.is_empty(),
            "must emit an UnknownPackage diagnostic for stale fixed group member; \
             got diagnostics: {:?}",
            agg.diagnostics
        );
    }

    fn linked_group(name: &str, members: &[PackageId]) -> GroupTable {
        let mut groups = GroupTable::default();
        let group_def = GroupDef {
            name: GroupName(name.to_string()),
            kind: GroupKind::Linked,
            members: members.iter().cloned().map(GroupMember::Package).collect(),
        };
        groups.linked.insert(group_def.name.clone(), group_def);
        groups
    }

    #[test]
    fn test_union_linked_propagates_severity_from_named_member() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let pkg_b = PackageId::parse("pkg-b").unwrap();

        let mut agg = Aggregation::default();
        agg.severities.insert(pkg_b.clone(), Severity::Minor);
        agg.named_by.insert(pkg_b.clone(), NamedBy::Changeset);

        let groups = linked_group("linked-pair", &[pkg_a.clone(), pkg_b.clone()]);

        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_a.clone(), Version::semver(1, 0, 0));
        base_versions.insert(pkg_b.clone(), Version::semver(1, 0, 0));

        let changed = union_linked(&mut agg, &groups, &base_versions);

        assert!(changed);
        assert_eq!(agg.severities.get(&pkg_a), Some(&Severity::Minor));
        assert_eq!(agg.severities.get(&pkg_b), Some(&Severity::Minor));
        assert_eq!(
            agg.reasons.get(&pkg_a),
            Some(&BumpReason::LinkedGroupUnion {
                group: GroupName("linked-pair".to_string()),
            })
        );
    }

    #[test]
    fn test_union_linked_does_not_downgrade_higher_existing_severity() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let pkg_b = PackageId::parse("pkg-b").unwrap();

        let mut agg = Aggregation::default();
        agg.severities.insert(pkg_a.clone(), Severity::Major);
        agg.severities.insert(pkg_b.clone(), Severity::Minor);
        agg.named_by.insert(pkg_a.clone(), NamedBy::Inference);
        agg.named_by.insert(pkg_b.clone(), NamedBy::Changeset);

        let groups = linked_group("linked-pair", &[pkg_a.clone(), pkg_b.clone()]);

        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_a.clone(), Version::semver(1, 0, 0));
        base_versions.insert(pkg_b.clone(), Version::semver(1, 0, 0));

        let changed = union_linked(&mut agg, &groups, &base_versions);

        assert!(changed);
        assert_eq!(agg.severities.get(&pkg_a), Some(&Severity::Major));
        assert_eq!(agg.severities.get(&pkg_b), Some(&Severity::Major));
    }

    #[test]
    fn test_union_linked_noop_when_no_member_named() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let pkg_b = PackageId::parse("pkg-b").unwrap();

        let mut agg = Aggregation::default();
        let groups = linked_group("linked-pair", &[pkg_a.clone(), pkg_b.clone()]);

        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_a.clone(), Version::semver(1, 0, 0));
        base_versions.insert(pkg_b.clone(), Version::semver(1, 0, 0));

        let changed = union_linked(&mut agg, &groups, &base_versions);

        assert!(!changed);
        assert!(agg.severities.is_empty());
    }

    /// Spec: when `SeverityInference::infer` returns `Err`, `aggregate()` must emit a
    /// diagnostic (warning level) describing the failure rather than silently discarding
    /// the error and leaving the package with no inferred severity bump.
    #[test]
    fn test_aggregate_inference_error_emits_diagnostic() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();

        init_repo(root);
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        let pkg_id = PackageId::parse("pkg-a").unwrap();
        let graph = SinglePackageGraph {
            pkg: make_pkg(pkg_id.clone()),
        };
        let cfg = crate::config::load(root).unwrap();
        let runner = RealGitRunner;
        let git = GitAccess::new(root, &runner);
        let tags = crate::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_id.clone(), Version::semver(1, 0, 0));

        struct AlwaysErrorInference;
        impl SeverityInference for AlwaysErrorInference {
            fn infer(
                &self,
                _pkg: &Package,
                _git: &GitAccess<'_>,
                _window: InferenceWindowSpec<'_>,
            ) -> Result<Option<InferenceOutcome>, GraphError> {
                Err(GraphError::Vcs(callisto_vcs::VcsError::Git(
                    "simulated inference failure".into(),
                )))
            }
        }

        let agg = aggregate(&graph, &cfg, &git, &tags, &base_versions, None, &AlwaysErrorInference).unwrap();

        assert!(
            !agg.diagnostics.is_empty(),
            "aggregate() must emit a diagnostic when SeverityInference::infer returns Err; got none"
        );
    }

    /// Spec: a bare-name `[[package]]` rule in callisto.toml must match a workspace
    /// package with a prefixed ID (e.g. `cargo/pkg-a`) via `PackageId::matches()`.
    /// The previous `id == &pkg.id` structural equality check was silently inert for
    /// prefixed package IDs when the config rule used a bare name.
    #[test]
    fn test_aggregate_bare_name_config_policy_matches_prefixed_package() {
        use crate::config::resolve::PreMajorInferencePolicy;
        use std::sync::atomic::AtomicBool;

        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();

        init_repo(root);
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        // Config uses a BARE name, but the workspace package has a PREFIXED ID.
        // PackageId::matches() must bridge the gap; == does not.
        std::fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"pkg-a\"\npre-major-inference = \"conservative\"\n",
        )
        .unwrap();

        let pkg_id = PackageId::parse("cargo/pkg-a").unwrap();
        let graph = SinglePackageGraph {
            pkg: make_pkg(pkg_id.clone()),
        };
        let cfg = crate::config::load(root).unwrap();
        let runner = RealGitRunner;
        let git = GitAccess::new(root, &runner);
        let tags = crate::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_id.clone(), Version::semver(0, 1, 0));

        struct PolicyCapturingInference2 {
            saw_non_off: AtomicBool,
        }
        impl SeverityInference for PolicyCapturingInference2 {
            fn infer(
                &self,
                _pkg: &Package,
                _git: &GitAccess<'_>,
                window: InferenceWindowSpec<'_>,
            ) -> Result<Option<InferenceOutcome>, GraphError> {
                if window.policy != PreMajorInferencePolicy::Off {
                    self.saw_non_off.store(true, Ordering::SeqCst);
                }
                Ok(None)
            }
        }

        let capturing = PolicyCapturingInference2 {
            saw_non_off: AtomicBool::new(false),
        };
        aggregate(&graph, &cfg, &git, &tags, &base_versions, None, &capturing).unwrap();

        assert!(
            capturing.saw_non_off.load(Ordering::SeqCst),
            "a bare-name [[package]] rule must match a prefixed package ID via \
             PackageId::matches(); the old == comparison was silently inert for \
             cargo/pkg-a when callisto.toml uses match = \"pkg-a\""
        );
    }

    /// Spec: during a pre-release cycle (PreMode::Pre), aggregate() must NOT populate
    /// agg.consumed. Changesets must remain on disk so they can be re-applied when the
    /// cycle exits. The previous code unconditionally pushed to consumed regardless of
    /// the PreState passed in.
    #[test]
    fn test_aggregate_does_not_consume_changesets_during_pre_mode() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();

        init_repo(root);
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        let cs_dir = root.join(".changeset");
        std::fs::create_dir_all(&cs_dir).unwrap();
        std::fs::write(
            cs_dir.join("some-feature.md"),
            "---\n\"pkg-a\": minor\n---\n\nA feature in pre mode.\n",
        )
        .unwrap();

        let pkg_id = PackageId::parse("pkg-a").unwrap();
        let graph = SinglePackageGraph {
            pkg: make_pkg(pkg_id.clone()),
        };
        let cfg = crate::config::load(root).unwrap();
        let runner = RealGitRunner;
        let git = GitAccess::new(root, &runner);
        let tags = crate::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_id.clone(), Version::semver(1, 0, 0));

        let pre_state = callisto_format::PreState::entering("next", [("pkg-a".to_string(), Version::semver(1, 0, 0))]);

        let inference = RecordingInference::default();
        let agg = aggregate(&graph, &cfg, &git, &tags, &base_versions, Some(&pre_state), &inference).unwrap();

        assert!(
            agg.consumed.is_empty(),
            "changesets must NOT be consumed during a pre-release cycle (PreMode::Pre); \
             agg.consumed must be empty but got: {:?}",
            agg.consumed
        );
    }

    /// Regression: the pre-mode changelog "from" baseline must be looked up
    /// in `pre.json`'s `initialVersions` by the same key `pre enter` writes
    /// it under (`PackageId::name()`, unqualified), not by `display_name()`
    /// (ecosystem-prefixed). For a `PackageId::Prefixed` id those differ, so
    /// a `display_name()` lookup always misses and falls through to
    /// `base_versions`/`0.0.0`, corrupting the changelog range for every
    /// ecosystem-qualified package in a pre-release cycle.
    #[test]
    fn test_aggregate_pre_mode_changelog_baseline_uses_pre_json_key_not_display_name() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();

        init_repo(root);
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        let cs_dir = root.join(".changeset");
        std::fs::create_dir_all(&cs_dir).unwrap();
        std::fs::write(
            cs_dir.join("some-feature.md"),
            "---\n\"npm/pkg-a\": minor\n---\n\nA feature in pre mode.\n",
        )
        .unwrap();

        let pkg_id = PackageId::parse("npm/pkg-a").unwrap();
        let graph = SinglePackageGraph {
            pkg: make_pkg(pkg_id.clone()),
        };
        let cfg = crate::config::load(root).unwrap();
        let runner = RealGitRunner;
        let git = GitAccess::new(root, &runner);
        let tags = crate::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
        let mut base_versions = BTreeMap::new();
        // Live on-disk version is far from the pinned pre-cycle baseline;
        // a key-lookup miss falling back to this would be caught below.
        base_versions.insert(pkg_id.clone(), Version::semver(9, 9, 9));

        // pre.json's initialVersions is keyed by the bare name ("pkg-a"),
        // exactly as `Workspace::initial_versions` (via `pre_json_key`) writes it.
        let pre_state = callisto_format::PreState::entering("next", [("pkg-a".to_string(), Version::semver(1, 2, 3))]);

        let inference = RecordingInference::default();
        let agg = aggregate(&graph, &cfg, &git, &tags, &base_versions, Some(&pre_state), &inference).unwrap();

        let cl_input = agg
            .changelog_inputs
            .get(&pkg_id)
            .expect("changeset entry must produce a changelog input");

        assert_eq!(
            cl_input.from.render(),
            "1.2.3",
            "pre-mode changelog baseline must resolve pre.json's pinned initialVersions \
             entry via the bare-name key, not fall through to base_versions (9.9.9) because \
             a display_name() lookup (\"npm/pkg-a\") missed the bare-name (\"pkg-a\") key"
        );
    }

    /// Spec: `aggregate()` must pass the per-package `pre_major_inference` policy from
    /// `config.packages` into `InferenceWindowSpec`, not always hardcode `OFF`.
    #[test]
    fn test_aggregate_pre_major_inference_policy_applied() {
        use crate::config::resolve::PreMajorInferencePolicy;
        use std::sync::atomic::AtomicBool;

        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();

        init_repo(root);
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        // Write a callisto.toml with pre_major_inference = "conservative" for pkg-a.
        // The [[package]] section requires a `match` field (pattern to match package names).
        std::fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"pkg-a\"\npre-major-inference = \"conservative\"\n",
        )
        .unwrap();

        let pkg_id = PackageId::parse("pkg-a").unwrap();
        let graph = SinglePackageGraph {
            pkg: make_pkg(pkg_id.clone()),
        };
        let cfg = crate::config::load(root).unwrap();
        let runner = RealGitRunner;
        let git = GitAccess::new(root, &runner);
        let tags = crate::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_id.clone(), Version::semver(0, 1, 0));

        // An inference impl that records whether it received a non-OFF policy.
        struct PolicyCapturingInference {
            saw_non_off: AtomicBool,
        }
        impl SeverityInference for PolicyCapturingInference {
            fn infer(
                &self,
                _pkg: &Package,
                _git: &GitAccess<'_>,
                window: InferenceWindowSpec<'_>,
            ) -> Result<Option<InferenceOutcome>, GraphError> {
                if window.policy != PreMajorInferencePolicy::Off {
                    self.saw_non_off.store(true, Ordering::SeqCst);
                }
                Ok(None)
            }
        }

        let capturing = PolicyCapturingInference {
            saw_non_off: AtomicBool::new(false),
        };
        aggregate(&graph, &cfg, &git, &tags, &base_versions, None, &capturing).unwrap();

        assert!(
            capturing.saw_non_off.load(Ordering::SeqCst),
            "aggregate() must pass the per-package pre_major_inference policy from config.packages \
             into InferenceWindowSpec; received OFF even though callisto.toml sets conservative"
        );
    }

    /// Prefixed rule (npm:pkg, OFF policy) must win over Bare rule (pkg, conservative)
    /// even when the Bare rule is declared first in callisto.toml.
    /// With single-pass lookup: the Bare rule (declared first) wins -> conservative applied.
    /// With resolve_package_config (two-pass): the Prefixed rule wins -> OFF applied.
    ///
    /// Two AtomicBool flags:
    /// - invoked: true if infer() was called at all (proves the package is pre-1.0 and
    ///   pre-major-inference was consulted; distinguishes OFF-applied from never-called).
    /// - saw_non_off: true if infer() received a non-OFF policy (single-pass failure mode).
    #[test]
    fn test_pre_major_inference_prefixed_beats_bare_when_bare_declared_first() {
        use crate::config::resolve::PreMajorInferencePolicy;
        use std::sync::atomic::AtomicBool;

        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();

        init_repo(root);
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        // Bare rule declared FIRST in TOML: match = "pkg", pre-major-inference = "conservative"
        // Prefixed rule declared SECOND: match = "npm:pkg", pre-major-inference = "off"
        // Single-pass find() returns pkg (Bare, declared first) -> conservative applied.
        // Two-pass resolve_package_config: pass 1 finds npm:pkg (Prefixed) -> OFF applied.
        std::fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"pkg\"\npre-major-inference = \"conservative\"\n\n[[package]]\nmatch = \"npm:pkg\"\npre-major-inference = \"off\"\n",
        )
        .unwrap();

        let pkg_id = PackageId::parse("pkg").unwrap();
        let graph = SinglePackageGraph {
            pkg: make_pkg(pkg_id.clone()),
        };
        let cfg = crate::config::load(root).unwrap();
        let runner = RealGitRunner;
        let git = GitAccess::new(root, &runner);
        let tags = crate::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
        let mut base_versions = BTreeMap::new();
        // Version 0.1.0 (pre-1.0) ensures pre-major-inference is consulted.
        base_versions.insert(pkg_id.clone(), Version::semver(0, 1, 0));

        struct PolicyCapturingInference {
            invoked: AtomicBool,
            saw_non_off: AtomicBool,
        }
        impl SeverityInference for PolicyCapturingInference {
            fn infer(
                &self,
                _pkg: &Package,
                _git: &GitAccess<'_>,
                window: InferenceWindowSpec<'_>,
            ) -> Result<Option<InferenceOutcome>, GraphError> {
                // Set invoked before any policy check so we can distinguish
                // OFF-policy-applied from never-called.
                self.invoked.store(true, Ordering::SeqCst);
                if window.policy != PreMajorInferencePolicy::Off {
                    self.saw_non_off.store(true, Ordering::SeqCst);
                }
                Ok(None)
            }
        }

        let capturing = PolicyCapturingInference {
            invoked: AtomicBool::new(false),
            saw_non_off: AtomicBool::new(false),
        };
        aggregate(&graph, &cfg, &git, &tags, &base_versions, None, &capturing).unwrap();

        assert!(
            capturing.invoked.load(Ordering::SeqCst),
            "inference was never invoked; fixture is wrong and cannot distinguish OFF policy \
             from no invocation. Ensure Version::semver(0, 1, 0) is in base_versions so the \
             package is pre-1.0 and pre-major-inference is consulted."
        );
        assert!(
            !capturing.saw_non_off.load(Ordering::SeqCst),
            "expected OFF policy from Prefixed rule (npm:pkg); Bare rule (conservative) was \
             applied instead. Two-pass specificity is required: Prefixed rules must win over \
             Bare rules regardless of declaration order."
        );
    }

    /// Spec: in pre mode, a changeset matched for the first time is recorded
    /// into `new_pre_changesets` (so the caller can persist its id into
    /// `pre.json`) and produces one changelog entry, but is never added to
    /// `consumed` (the file must stay on disk). Reproduces the
    /// ver-pre-reapply bug: previously `consumed` was the only record of
    /// "seen this run", and pre mode always suppressed it, so a rerun had no
    /// way to tell an already-applied changeset from a new one.
    #[test]
    fn test_pre_mode_first_match_is_recorded_not_consumed() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        let cs_dir = root.join(".changeset");
        std::fs::create_dir_all(&cs_dir).unwrap();
        std::fs::write(
            cs_dir.join("cool-thing.md"),
            "---\npkg-a: minor\n---\n\nAdds a thing.\n",
        )
        .unwrap();
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        let pkg_id = PackageId::parse("pkg-a").unwrap();
        let graph = SinglePackageGraph {
            pkg: make_pkg(pkg_id.clone()),
        };
        let cfg = crate::config::load(root).unwrap();
        let runner = RealGitRunner;
        let git = GitAccess::new(root, &runner);
        let tags = crate::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_id.clone(), Version::semver(0, 1, 0));

        let mut initial_versions = indexmap::IndexMap::new();
        initial_versions.insert("pkg-a".to_string(), Version::semver(0, 1, 0));
        let pre_state = callisto_format::PreState {
            mode: callisto_format::PreMode::Pre,
            tag: "beta".to_string(),
            initial_versions,
            changesets: Vec::new(),
        };

        let inference = crate::infer::NoInference;
        let agg = aggregate(&graph, &cfg, &git, &tags, &base_versions, Some(&pre_state), &inference).unwrap();

        assert_eq!(
            agg.new_pre_changesets,
            vec!["cool-thing".to_string()],
            "a changeset matched for the first time in pre mode must be recorded as new"
        );
        assert!(
            agg.consumed.is_empty(),
            "pre mode must never add a changeset to `consumed` (would delete it from disk); got: {:?}",
            agg.consumed
        );
        assert_eq!(
            agg.changelog_inputs.get(&pkg_id).map(|i| i.entries.len()),
            Some(1),
            "a newly-seen pre-mode changeset must produce exactly one changelog entry"
        );
    }

    /// Spec: once a changeset's id is already in `pre.json`'s `changesets`
    /// list, a rerun must still count its severity (so cascade computes the
    /// correct target against `initialVersions`) but must NOT record it
    /// again as new and must NOT add a second changelog entry -- otherwise
    /// every `version` run during a pre-release cycle re-applies the same
    /// changeset, duplicating its changelog line (the ver-pre-reapply bug).
    #[test]
    fn test_pre_mode_already_recorded_changeset_counts_severity_but_not_changelog() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);

        let cs_dir = root.join(".changeset");
        std::fs::create_dir_all(&cs_dir).unwrap();
        std::fs::write(
            cs_dir.join("cool-thing.md"),
            "---\npkg-a: minor\n---\n\nAdds a thing.\n",
        )
        .unwrap();
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        let pkg_id = PackageId::parse("pkg-a").unwrap();
        let graph = SinglePackageGraph {
            pkg: make_pkg(pkg_id.clone()),
        };
        let cfg = crate::config::load(root).unwrap();
        let runner = RealGitRunner;
        let git = GitAccess::new(root, &runner);
        let tags = crate::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_id.clone(), Version::semver(0, 1, 0));

        let mut initial_versions = indexmap::IndexMap::new();
        initial_versions.insert("pkg-a".to_string(), Version::semver(0, 1, 0));
        let pre_state = callisto_format::PreState {
            mode: callisto_format::PreMode::Pre,
            tag: "beta".to_string(),
            initial_versions,
            // Simulates a prior `version` run already having recorded this id.
            changesets: vec!["cool-thing".to_string()],
        };

        let inference = crate::infer::NoInference;
        let agg = aggregate(&graph, &cfg, &git, &tags, &base_versions, Some(&pre_state), &inference).unwrap();

        assert!(
            agg.new_pre_changesets.is_empty(),
            "an already-recorded pre-mode changeset must not be reported as new; got: {:?}",
            agg.new_pre_changesets
        );
        assert!(
            agg.consumed.is_empty(),
            "pre mode must never delete a changeset from disk, recorded or not; got: {:?}",
            agg.consumed
        );
        assert_eq!(
            agg.severities.get(&pkg_id),
            Some(&Severity::Minor),
            "an already-recorded changeset must still count toward severity against initialVersions"
        );
        assert!(
            !agg.changelog_inputs.contains_key(&pkg_id),
            "an already-recorded pre-mode changeset must not produce a new changelog entry on rerun; \
             got: {:?}",
            agg.changelog_inputs.get(&pkg_id)
        );
        assert!(
            agg.pre_mode_has_active_changeset,
            "an already-recorded changeset still actively drives this run (severity + prerelease \
             counter), so pre_mode_has_active_changeset must be true, not just new_pre_changesets"
        );
    }

    /// Spec: with no changeset on disk at all in pre mode (neither new nor
    /// previously recorded), the run has no active changeset -- this, not
    /// `new_pre_changesets` alone, is what must gate the "no pending
    /// changesets" warning: a rerun with an
    /// already-recorded changeset still driving severity is NOT the same as
    /// a genuinely empty run.
    #[test]
    fn test_pre_mode_no_changesets_at_all_has_no_active_changeset() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();
        init_repo(root);
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        let pkg_id = PackageId::parse("pkg-a").unwrap();
        let graph = SinglePackageGraph {
            pkg: make_pkg(pkg_id.clone()),
        };
        let cfg = crate::config::load(root).unwrap();
        let runner = RealGitRunner;
        let git = GitAccess::new(root, &runner);
        let tags = crate::tags::TagIndex::build(&git, &graph, &cfg).unwrap();
        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_id, Version::semver(0, 1, 0));

        let mut initial_versions = indexmap::IndexMap::new();
        initial_versions.insert("pkg-a".to_string(), Version::semver(0, 1, 0));
        let pre_state = callisto_format::PreState {
            mode: callisto_format::PreMode::Pre,
            tag: "beta".to_string(),
            initial_versions,
            changesets: Vec::new(),
        };

        let inference = crate::infer::NoInference;
        let agg = aggregate(&graph, &cfg, &git, &tags, &base_versions, Some(&pre_state), &inference).unwrap();

        assert!(!agg.pre_mode_has_active_changeset);
        assert!(agg.new_pre_changesets.is_empty());
    }
}
