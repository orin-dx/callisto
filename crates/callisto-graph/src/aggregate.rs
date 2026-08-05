use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use callisto_changelog::{ChangeSource, ChangelogEntry, ChangelogInput};
use callisto_format::{parse_changeset, Changeset};
use callisto_model::{
    BumpReason, CommandRunner, CommitSha, Diagnostic, Package, PackageId, Severity, Version,
};
use callisto_vcs::{GitAccess, GitDataSource};

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
    pub consumed: Vec<PathBuf>,
    pub changelog_inputs: BTreeMap<PackageId, ChangelogInput>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedBy {
    Changeset,
    Inference,
}

pub fn load_changesets(
    root: &Path,
    cfg: &ResolvedConfig,
) -> Result<Vec<LoadedChangeset>, GraphError> {
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
                if file_name != "README.md" && file_name != "config.json" && file_name != "pre.json"
                {
                    files.push(path);
                }
            }
        }
    }

    files.sort();

    let mut loaded = Vec::new();
    for path in files {
        let content =
            fs::read_to_string(&path).map_err(|e| callisto_model::ManifestError::Read {
                path: path.clone(),
                message: e.to_string(),
            })?;
        let changeset = parse_changeset(&content).map_err(|e| GraphError::ParseChangeset {
            path: path.clone(),
            source: e,
        })?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
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
    if policy == PreMajorInferencePolicy::OFF {
        return (inferred, false);
    }
    if current.major() != Some(0) || current.minor() == Some(0) || !has_prior_release {
        return (inferred, false);
    }

    match (policy, inferred) {
        (p, Severity::Major) if p.breaking_to_minor => (Severity::Minor, true),
        (p, Severity::Minor) if p.feat_to_patch => (Severity::Patch, true),
        (_, s) => (s, false),
    }
}

/// Resolves a release tag name to the commit SHA it points at, so that
/// severity inference can be scoped to `since..HEAD` instead of walking the
/// entire history on every `aggregate()`-driven command.
///
/// Thin wrapper around [`GitDataSource::resolve_commit`] (native gix,
/// falling back to a `CommandRunner`-shelled `git rev-parse` when gix is
/// unavailable -- most notably on `wasm32`): any failure to resolve the tag
/// (missing, unborn repo, etc.) degrades gracefully to `None`, which
/// callers treat as "infer over full history" -- the same behavior this
/// function has always had, now delegated to [`GitAccess`] instead of
/// hand-rolling the gix-then-runner-fallback shape itself.
fn resolve_since(git: &impl GitDataSource, tag_name: &str) -> Option<CommitSha> {
    git.resolve_commit(tag_name).ok().flatten()
}

/// Resolves a changeset entry's parsed `PackageId` against the packages in
/// the graph.
///
/// `PackageId::matches` is a pairwise compatibility check: a bare id and a
/// prefixed id with the same name are considered compatible because the
/// bare side simply doesn't specify an ecosystem. That's correct pairwise,
/// but a polyglot workspace can legitimately contain the same name in two
/// or more ecosystems (e.g. `cargo/foo` and `npm/foo`), and a bare
/// changeset entry naming `foo` cannot be resolved to either one without
/// more context. Iterating with `.find()` over such a graph silently picks
/// whichever candidate happens to come first, which is exactly the
/// ambiguity bug this function fixes: it collects *all* matching
/// candidates and only succeeds when there is exactly one.
///
/// Returns `Ok(None)` when no package matches (an unknown-package
/// condition, reported separately by `validate`), `Ok(Some(pkg))` when
/// resolution is unambiguous, and `Err(GraphError::AmbiguousName)` when the
/// id matches two or more packages.
pub(crate) fn resolve_target_package<'a>(
    packages: impl Iterator<Item = &'a Package>,
    id: &PackageId,
) -> Result<Option<&'a Package>, GraphError> {
    let matching: Vec<&Package> = packages.filter(|p| p.id.matches(id)).collect();
    match matching.len() {
        0 => Ok(None),
        1 => Ok(Some(matching[0])),
        _ => Err(GraphError::AmbiguousName {
            name: id.display_name(),
            candidates: matching.iter().map(|p| p.id.clone()).collect(),
        }),
    }
}

pub fn aggregate<D, R, I>(
    graph: &D,
    config: &ResolvedConfig,
    runner: &R,
    tags: &TagIndex,
    base_versions: &BTreeMap<PackageId, Version>,
    _pre: Option<&callisto_format::PreState>,
    inference: &I,
) -> Result<Aggregation, GraphError>
where
    D: DependencyResolver,
    R: CommandRunner,
    I: SeverityInference,
{
    let loaded = load_changesets(&config.root, config)?;
    let mut agg = Aggregation::default();

    // Constructed once and reused for every package's since-resolution
    // below (native gix, falling back to `runner` when unavailable); a
    // resolution failure degrades gracefully to `None`, same as
    // `resolve_since`'s own per-tag failure handling.
    let git = GitAccess::discover(&config.root, runner);

    for pkg in graph.packages() {
        let cur_sev = agg
            .severities
            .get(&pkg.id)
            .copied()
            .unwrap_or(Severity::None);
        let pathspecs: Vec<PathBuf> = pkg.manifests.iter().map(|m| m.path.clone()).collect();
        let last_tag = tags.last_tag(&pkg.id);
        let cur_ver = last_tag
            .map(|t| t.version.clone())
            .or_else(|| base_versions.get(&pkg.id).cloned())
            .ok_or_else(|| {
                GraphError::Manifest(callisto_model::ManifestError::MissingField {
                    path: pkg
                        .manifests
                        .first()
                        .map(|m| m.path.clone())
                        .unwrap_or_default(),
                    field: "version",
                })
            })?;

        let since = last_tag.and_then(|t| resolve_since(&git, t.name.as_str()));

        let policy = config
            .packages
            .iter()
            .find(|(id, _)| id == &pkg.id)
            .and_then(|(_, pcfg)| pcfg.pre_major_inference)
            .unwrap_or(PreMajorInferencePolicy::OFF);

        let window = crate::infer::InferenceWindowSpec {
            pathspecs: &pathspecs,
            since,
            current_version: &cur_ver,
            has_prior_release: last_tag.is_some(),
            policy,
        };

        match inference.infer(pkg, window) {
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
                }
            }
            Ok(None) => {}
            Err(e) => {
                agg.diagnostics.push(Diagnostic {
                    code: callisto_model::DiagnosticCode::PreMajorInferenceInert,
                    severity: callisto_model::DiagnosticSeverity::Warning,
                    message: format!(
                        "Commit inference failed for package `{}`: {e}",
                        pkg.id.display_name()
                    ),
                    package: Some(pkg.id.clone()),
                    path: None,
                    governed_by: None,
                    escalated_by: None,
                });
            }
        }
    }

    for cs in loaded {
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
                    let cur_sev = agg
                        .severities
                        .get(&canonical_id)
                        .copied()
                        .unwrap_or(Severity::None);
                    if entry.severity > cur_sev {
                        agg.severities.insert(canonical_id.clone(), entry.severity);
                        agg.reasons.insert(
                            canonical_id.clone(),
                            BumpReason::Changeset {
                                changesets: vec![cs.id.clone()],
                            },
                        );
                        agg.named_by
                            .insert(canonical_id.clone(), NamedBy::Changeset);
                    }

                    if entry.severity != Severity::None {
                        let pkg_ver = tags
                            .last_tag(&canonical_id)
                            .map(|t| t.version.clone())
                            .or_else(|| base_versions.get(&canonical_id).cloned())
                            .unwrap_or_else(|| Version::semver(0, 0, 0));
                        let cl_input = agg
                            .changelog_inputs
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
        // Only mark as consumed when at least one entry resolved to a real
        // package.  A fully-orphaned changeset is left on disk.
        if matched_any {
            agg.consumed.push(cs.path.clone());
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
        let pkg_members: Vec<PackageId> = g
            .members(crate::config::GroupMemberKind::Package)
            .filter_map(|m| match m {
                crate::config::GroupMember::Package(ref id) => Some(id.clone()),
                _ => None,
            })
            .collect();

        let mut target = Severity::None;
        for m in &pkg_members {
            if let Some(&s) = agg.severities.get(m) {
                if s > target {
                    target = s;
                }
            }
        }

        if target == Severity::None {
            continue;
        }

        for m in pkg_members {
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
                agg.reasons.insert(
                    m.clone(),
                    BumpReason::FixedGroupUnion {
                        group: g.name.clone(),
                    },
                );
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
        let named: Vec<PackageId> = g
            .members(crate::config::GroupMemberKind::Package)
            .filter_map(|m| match m {
                crate::config::GroupMember::Package(ref id) => {
                    if agg.named_by.contains_key(id) {
                        Some(id.clone())
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect();

        if named.is_empty() {
            continue;
        }

        let mut target_sev = Severity::None;
        for m in &named {
            if let Some(&s) = agg.severities.get(m) {
                if s > target_sev {
                    target_sev = s;
                }
            }
        }

        let all_members: Vec<PackageId> = g
            .members(crate::config::GroupMemberKind::Package)
            .filter_map(|m| match m {
                crate::config::GroupMember::Package(ref id) => Some(id.clone()),
                _ => None,
            })
            .collect();

        for m in all_members {
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
                agg.reasons.insert(
                    m.clone(),
                    BumpReason::LinkedGroupUnion {
                        group: g.name.clone(),
                    },
                );
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
        CommandError, CommandOutput, DepEdge, GroupKind, GroupName, ManifestDecl, ManifestFormat,
        ManifestRole, Package,
    };

    use crate::config::{GroupDef, GroupMember};
    use crate::infer::{InferenceOutcome, InferenceWindowSpec, SeverityInference};
    use callisto_fixtures::git::{init_repo, run_git, PoisonedRunner};

    /// Shells out to the real `git` binary. Retained as the `CommandRunner`
    /// implementation passed to `aggregate()`/`TagIndex::build` in most
    /// tests below, even though neither actually uses it for git access
    /// anymore: both resolve against the real repo on disk via
    /// `callisto_vcs::GitRepository` (gix). See
    /// `test_aggregate_resolves_since_without_shelling_through_runner` for
    /// the test proving `aggregate()`'s since-resolution no longer needs a
    /// working runner at all.
    struct RealGitRunner;

    impl CommandRunner for RealGitRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            cwd: &Path,
        ) -> Result<CommandOutput, CommandError> {
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

    /// A directory that is guaranteed not to sit inside any Git repository,
    /// so `callisto_vcs::GitRepository::discover` fails exactly the way it
    /// unconditionally does on `wasm32` -- the native-testable stand-in for
    /// "gix is unavailable" used to force `resolve_since` through its
    /// `CommandRunner` fallback. Mirrors `tags.rs`'s helper of the same
    /// name.
    fn non_repo_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            callisto_vcs::GitRepository::discover(dir.path()).is_err(),
            "test fixture must not be discoverable as a Git repo"
        );
        dir
    }

    /// A `CommandRunner` double that answers `git rev-parse --verify --quiet
    /// <tag>^{commit}` with a canned SHA and counts invocations. Stands in
    /// for the real `git` binary on the `resolve_since` fallback path,
    /// exercised when gix is unavailable (`repo: None`, as is permanently
    /// the case on `wasm32`).
    struct FakeRevParseRunner {
        calls: AtomicUsize,
        tag: String,
        sha: CommitSha,
    }

    impl CommandRunner for FakeRevParseRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &Path,
        ) -> Result<CommandOutput, CommandError> {
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
        std::fs::write(
            cs_dir.join("malformed-changeset.md"),
            "cargo/foo: patch\n\nSummary.\n",
        )
        .unwrap();

        let cfg = crate::config::load(root).unwrap();
        let result = load_changesets(root, &cfg);

        let err =
            result.expect_err("load_changesets must return Err for a malformed changeset file");
        let err_display = format!("{err}");
        assert!(
            err_display.contains("malformed-changeset"),
            "error message must contain the offending filename so the developer can triage; \
             got: {err_display:?}"
        );
    }

    /// Spec: `resolve_since` must not silently degrade to `None` (forcing
    /// an unbounded full-history commit walk, see
    /// `test_aggregate_scopes_inference_window_to_last_tag`) just because
    /// gix is unavailable -- it must fall back (via `GitAccess`) to a
    /// `CommandRunner`-shelled `git rev-parse --verify --quiet
    /// <tag>^{commit}` call.
    #[test]
    fn test_resolve_since_falls_back_to_command_runner_without_gix() {
        let dir = non_repo_dir();
        let sha = CommitSha::parse("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef").unwrap();
        let runner = FakeRevParseRunner {
            calls: AtomicUsize::new(0),
            tag: "pkg-a@1.0.0".to_string(),
            sha: sha.clone(),
        };
        let git = GitAccess::discover(dir.path(), &runner);

        let resolved = resolve_since(&git, "pkg-a@1.0.0");

        assert_eq!(
            resolved,
            Some(sha),
            "resolve_since must resolve the tag via the CommandRunner fallback when gix is \
             unavailable, not silently return None"
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
        run_git(
            root,
            &["-c", "tag.gpgSign=false", "tag", "-m", "release", &tag_name],
        );

        // A commit landing after the tag; a correctly-scoped inference
        // window must never need to look past `tag_name` to find it, but a
        // `since: None` (full history) window would happily walk right over
        // it and beyond, all the way back to the repo root.
        std::fs::write(root.join("CHANGES.md"), "more\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: add changes file"]);

        let expected_sha_output = std::process::Command::new("git")
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{tag_name}^{{commit}}"),
            ])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(expected_sha_output.status.success());
        let expected_sha =
            CommitSha::parse(String::from_utf8_lossy(&expected_sha_output.stdout).trim()).unwrap();

        let runner = RealGitRunner;
        let manifest = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
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
        let cfg = crate::config::load(root).unwrap();
        let tags = TagIndex::build(&runner, root, &graph, &cfg).unwrap();

        // Sanity: the tag we just created was actually picked up.
        assert_eq!(
            tags.last_tag(&pkg_id)
                .map(|t| t.version.render().to_string()),
            Some("1.0.0".to_string())
        );

        let inference = RecordingInference::default();
        let base_versions = BTreeMap::new();

        aggregate(
            &graph,
            &cfg,
            &runner,
            &tags,
            &base_versions,
            None,
            &inference,
        )
        .unwrap();

        let captured = inference.captured_since.lock().unwrap().clone();
        assert_eq!(
            captured,
            Some(expected_sha),
            "aggregate() must scope inference to last_tag..HEAD instead of hardcoding `since: None` \
             (full history)"
        );
    }

    /// Spec: since-resolution must go through `callisto_vcs::GitRepository`
    /// (gix), not the `CommandRunner` shell-out -- a `CommandRunner` that
    /// fails on every call must not prevent `since` from being resolved.
    #[test]
    fn test_aggregate_resolves_since_without_shelling_through_runner() {
        let ws_dir = tempfile::tempdir().unwrap();
        let root = ws_dir.path();

        init_repo(root);
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "initial commit"]);

        let pkg_id = PackageId::parse("pkg-a").unwrap();
        let tag_name = format!("{}@1.0.0", pkg_id.display_name());
        run_git(
            root,
            &["-c", "tag.gpgSign=false", "tag", "-m", "release", &tag_name],
        );

        std::fs::write(root.join("CHANGES.md"), "more\n").unwrap();
        run_git(root, &["add", "."]);
        run_git(root, &["commit", "-q", "-m", "feat: add changes file"]);

        let expected_sha_output = std::process::Command::new("git")
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{tag_name}^{{commit}}"),
            ])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(expected_sha_output.status.success());
        let expected_sha =
            CommitSha::parse(String::from_utf8_lossy(&expected_sha_output.stdout).trim()).unwrap();

        let poisoned = PoisonedRunner;
        let manifest = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
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
        let cfg = crate::config::load(root).unwrap();
        let tags = TagIndex::build(&poisoned, root, &graph, &cfg).unwrap();

        assert_eq!(
            tags.last_tag(&pkg_id)
                .map(|t| t.version.render().to_string()),
            Some("1.0.0".to_string())
        );

        let inference = RecordingInference::default();
        let base_versions = BTreeMap::new();

        aggregate(
            &graph,
            &cfg,
            &poisoned,
            &tags,
            &base_versions,
            None,
            &inference,
        )
        .unwrap();

        let captured = inference.captured_since.lock().unwrap().clone();
        assert_eq!(
            captured,
            Some(expected_sha),
            "aggregate() must resolve `since` via callisto_vcs::GitRepository (gix), not by \
             shelling out through the CommandRunner"
        );
    }

    fn make_pkg(id: PackageId) -> Package {
        let manifest = ManifestDecl::new(
            "Cargo.toml",
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        Package {
            id,
            manifests: vec![manifest],
            changelog: None,
            release_trigger: callisto_model::ReleaseTrigger::Changeset,
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
        let tags = crate::tags::TagIndex::build(&runner, root, &graph, &cfg).unwrap();

        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_bar_id.clone(), Version::semver(1, 0, 0));

        let inference = RecordingInference::default();
        let agg = aggregate(
            &graph,
            &cfg,
            &runner,
            &tags,
            &base_versions,
            None,
            &inference,
        )
        .unwrap();

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

    /// Spec: when a fixed group in callisto.toml references a package that no
    /// longer exists in the workspace, `union_fixed` must NOT insert the stale
    /// member into `agg.severities`.  Doing so causes `bump_target` in
    /// `solve_cascade` to call `input.base.get(stale_id)` -> `None` ->
    /// `Err(GraphError::Manifest(MissingField))`, crashing `callisto version`
    /// with a misleading error.  After the fix the stale member must be
    /// skipped and an `UnknownPackage` warning must be emitted.
    ///
    /// Setup: `pkg_bar` has `Severity::Minor` (from a changeset), `pkg_baz`
    /// has no severity yet.  Fixed group contains all three: `pkg_foo`
    /// (stale), `pkg_bar`, `pkg_baz`.  `union_fixed` should propagate `Minor`
    /// to `pkg_baz` (real member), skip `pkg_foo` with a diagnostic, and
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
        let tags = crate::tags::TagIndex::build(&runner, root, &graph, &cfg).unwrap();
        let mut base_versions = BTreeMap::new();
        base_versions.insert(pkg_id.clone(), Version::semver(1, 0, 0));

        struct AlwaysErrorInference;
        impl SeverityInference for AlwaysErrorInference {
            fn infer(
                &self,
                _pkg: &Package,
                _window: InferenceWindowSpec<'_>,
            ) -> Result<Option<InferenceOutcome>, GraphError> {
                Err(GraphError::Vcs(callisto_vcs::VcsError::Git(
                    "simulated inference failure".into(),
                )))
            }
        }

        let agg = aggregate(
            &graph,
            &cfg,
            &runner,
            &tags,
            &base_versions,
            None,
            &AlwaysErrorInference,
        )
        .unwrap();

        assert!(
            !agg.diagnostics.is_empty(),
            "aggregate() must emit a diagnostic when SeverityInference::infer returns Err; got none"
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
        let tags = crate::tags::TagIndex::build(&runner, root, &graph, &cfg).unwrap();
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
                window: InferenceWindowSpec<'_>,
            ) -> Result<Option<InferenceOutcome>, GraphError> {
                if window.policy != PreMajorInferencePolicy::OFF {
                    self.saw_non_off.store(true, Ordering::SeqCst);
                }
                Ok(None)
            }
        }

        let capturing = PolicyCapturingInference {
            saw_non_off: AtomicBool::new(false),
        };
        aggregate(
            &graph,
            &cfg,
            &runner,
            &tags,
            &base_versions,
            None,
            &capturing,
        )
        .unwrap();

        assert!(
            capturing.saw_non_off.load(Ordering::SeqCst),
            "aggregate() must pass the per-package pre_major_inference policy from config.packages \
             into InferenceWindowSpec; received OFF even though callisto.toml sets conservative"
        );
    }
}
