//! Deterministic, graph-owned release roster decisions.
//!
//! This is deliberately separate from `PublishPlan`: it records the exact
//! package/version authority that later intent construction consumes, without
//! exposing a mutation route.

use callisto_model::{
    BumpReason, CommandRunner, CommitSha, Ecosystem, Package, ReleaseDecisionEntry, ReleaseDecisionV1,
    ReleaseInclusionReason, ReleasePackageId, Version,
};

use crate::{DependencyResolver, GraphError, VersionPlan, Workspace};

/// Computes one [`ReleasePackageId`] per canonical manifest, ecosystem-qualified
/// against `package`'s name.
///
/// This is the single derivation of release package identity shared by every
/// site that computes or verifies release authority: [`derive_release_decision`],
/// [`derive_release_commit_decision`], and `release::derive_release_inputs`.
/// Per this module's own established lesson (see the doc comment on
/// [`derive_release_commit_decision`]), independently reimplementing this
/// mapping at each call site is exactly the kind of duplicated derivation
/// that drifts -- so it lives here once instead.
pub(crate) fn release_package_ids(package: &Package) -> Result<Vec<ReleasePackageId>, GraphError> {
    package
        .canonical_manifests()
        .map(|manifest| ReleasePackageId::new(manifest.ecosystem(), package.id.name()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_error| GraphError::ReleaseIntentStale)
}

/// Derives the durable roster from a freshly computed version plan.
///
/// The caller supplies the plan from the same workspace observation; this
/// function never inspects `PublishPlan` or a caller-provided release roster.
pub fn derive_release_decision<R: callisto_model::CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    plan: &VersionPlan,
) -> Result<ReleaseDecisionV1, GraphError> {
    let mut package_ids = std::collections::BTreeMap::new();
    for package in workspace.graph.packages() {
        let ids = release_package_ids(package)?;
        package_ids.insert(package.id.clone(), ids);
    }

    let mut entries = Vec::new();
    for bump in &plan.bumps {
        let ids = package_ids.get(&bump.package).ok_or(GraphError::ReleaseIntentStale)?;
        for id in ids {
            entries.push(ReleaseDecisionEntry {
                package: id.clone(),
                target_version: bump.to.clone(),
                reasons: vec![reason_from_bump(bump.reason.as_ref(), &package_ids)?],
            });
        }
    }
    ReleaseDecisionV1::new(entries).map_err(|_error| GraphError::ReleaseIntentStale)
}

/// Derives a durable decision for explicit, exact release identities.
///
/// A linked group is one release unit: selecting any member includes every
/// member of that linked group which the version plan selected. All other
/// packages remain outside the authority boundary. The caller must pass
/// ecosystem-qualified [`ReleasePackageId`] values; this function never uses
/// `PackageId::matches`, whose bare-name wildcard semantics are unsuitable
/// for release authority.
pub fn derive_selected_release_decision<R: callisto_model::CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    plan: &VersionPlan,
    selections: &[ReleasePackageId],
) -> Result<ReleaseDecisionV1, GraphError> {
    let complete = derive_release_decision(workspace, plan)?;
    let selected = selections.iter().collect::<std::collections::BTreeSet<_>>();
    if selected.len() != selections.len() {
        return Err(GraphError::ReleaseIntentStale);
    }
    if selected
        .iter()
        .any(|selection| !complete.entries.iter().any(|entry| &entry.package == *selection))
    {
        return Err(GraphError::ReleaseIntentStale);
    }

    let linked_groups = complete
        .entries
        .iter()
        .filter(|entry| selected.contains(&entry.package))
        .flat_map(|entry| entry.reasons.iter())
        .filter_map(|reason| match reason {
            ReleaseInclusionReason::LinkedGroup { group_id } => Some(group_id.clone()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();

    let entries = complete
        .entries
        .into_iter()
        .filter(|entry| {
            selected.contains(&entry.package)
                || entry.reasons.iter().any(|reason| {
                    matches!(reason, ReleaseInclusionReason::LinkedGroup { group_id } if linked_groups.contains(group_id))
                })
        })
        .collect();
    ReleaseDecisionV1::new(entries).map_err(|_error| GraphError::ReleaseIntentStale)
}

/// Verifies the release roster a merged release commit claims, against a
/// release-decision file committed alongside it at `decision_path`.
///
/// This is intentionally *not* a second computation of changeset, fixed-group,
/// linked-group, cascade, or pre-release-policy inclusion. `plan_version`
/// (via [`derive_release_decision`]) already computed that once, correctly,
/// when the release PR was generated -- and `callisto version --emit-decision`
/// commits its exact output alongside the manifest and changelog edits.
/// Re-deriving that policy a second time here, from raw git diffs, is exactly
/// the kind of duplicated logic that drifts: an earlier version of this
/// function did just that, understood only a direct changeset match, and
/// rejected every real release in this repository once a fixed-group cascade
/// (a case its reimplementation never learned) touched an unnamed sibling.
///
/// Instead, this function reads the committed decision back and confirms the
/// commit's actual diff matches it exactly -- no more, no less.
/// [`ReleaseDecisionV1`]'s own deserializer already rejects a decision whose
/// entries don't match its content digest, so a hand-edited or corrupted
/// decision file fails before this function's own diff cross-check runs.
///
/// The caller must check out the exact merge commit in detached HEAD state
/// before creating an intent. GitHub-specific PR/approval provenance belongs
/// in the workflow boundary; this graph function verifies the local,
/// provider-neutral commit delta only.
pub fn derive_release_commit_decision<R: CommandRunner, D: DependencyResolver>(
    workspace: &Workspace<'_, R, D>,
    release_commit: &CommitSha,
    decision_path: &std::path::Path,
) -> Result<ReleaseDecisionV1, GraphError> {
    let head = git_stdout(workspace.runner, &workspace.root, &["rev-parse", "HEAD"])?;
    let head = CommitSha::parse(&head).map_err(|_error| GraphError::ReleaseIntentStale)?;
    if &head != release_commit {
        return Err(GraphError::ReleaseIntentStale);
    }

    let parent_ref = format!("{}^", release_commit.as_str());
    let parent = git_stdout(workspace.runner, &workspace.root, &["rev-parse", &parent_ref])?;
    CommitSha::parse(&parent).map_err(|_error| GraphError::ReleaseIntentStale)?;

    let changed = git_stdout(
        workspace.runner,
        &workspace.root,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-status",
            "-r",
            "--no-renames",
            &parent,
            release_commit.as_str(),
        ],
    )?;
    let changed = parse_name_status(&changed)?;

    // The decision must be freshly authored as part of *this* commit, not a
    // stale leftover an earlier release already committed and this one never
    // touched -- otherwise a commit that changes nothing real could still
    // carry forward a prior, unrelated decision's authority.
    let decision_path_str = decision_path.to_string_lossy().replace('\\', "/");
    let decision_freshly_written = changed
        .iter()
        .any(|(status, path)| path == &decision_path_str && matches!(status.as_str(), "A" | "M"));
    if !decision_freshly_written {
        return Err(GraphError::ReleaseIntentStale);
    }

    // A cheap, independent sanity check that this commit is a real version
    // application and not just a hand-crafted decision file: some real
    // changeset was actually consumed. The decision's own entries, not this
    // changeset's content, remain the sole authority verified below.
    let changeset_dir = workspace.config.changesets_dir.to_string_lossy().replace('\\', "/");
    let changeset_prefix = format!("{}/", changeset_dir.trim_end_matches('/'));
    let consumed_a_changeset = changed
        .iter()
        .any(|(status, path)| status == "D" && path.starts_with(&changeset_prefix) && path.ends_with(".md"));
    if !consumed_a_changeset {
        return Err(GraphError::ReleaseIntentStale);
    }

    let decision_source = git_file(
        workspace.runner,
        &workspace.root,
        release_commit.as_str(),
        &decision_path_str,
    )?;
    let decision: ReleaseDecisionV1 =
        serde_json::from_str(&decision_source).map_err(|_error| GraphError::ReleaseIntentStale)?;
    let claimed = decision
        .entries
        .iter()
        .map(|entry| (entry.package.clone(), entry.target_version.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();

    let changed_paths = changed
        .iter()
        .filter_map(|(status, path)| matches!(status.as_str(), "A" | "M").then_some(path.as_str()))
        .collect::<std::collections::BTreeSet<_>>();

    // Every canonical manifest path this workspace currently declares, in
    // the exact order `workspace.graph.packages()` and
    // `Package::canonical_manifests()` yield -- both are backed by ordered
    // (`BTreeMap`/`Vec`) storage with no interior mutation between calls, so
    // the second traversal below visits the same (package, manifest) pairs
    // in the same order. That lets it zip the batched `before`/`after`
    // results back on by position instead of re-keying by path, which would
    // misbehave if two packages ever declared an identical manifest path.
    let manifest_queries: Vec<(String, Ecosystem)> = workspace
        .graph
        .packages()
        .flat_map(Package::canonical_manifests)
        .map(|manifest| (manifest.path.to_string_lossy().into_owned(), manifest.ecosystem()))
        .collect();
    let query_paths: Vec<&str> = manifest_queries.iter().map(|(path, _)| path.as_str()).collect();

    // Exactly two `git cat-file --batch` subprocess invocations total (one
    // per commit), regardless of how many canonical manifests the
    // workspace has -- replaces what was previously 2*N separate `git
    // show` spawns (one `manifest_version_at` call per manifest per
    // commit).
    let before_blobs = batch_manifest_blobs_at(workspace.runner, &workspace.root, &parent, &query_paths)?;
    let after_blobs =
        batch_manifest_blobs_at(workspace.runner, &workspace.root, release_commit.as_str(), &query_paths)?;
    let before_versions = resolve_batch_versions(before_blobs, &manifest_queries)?;
    let after_versions = resolve_batch_versions(after_blobs, &manifest_queries)?;

    let mut observed = std::collections::BTreeSet::new();
    let mut manifest_index = 0usize;
    for package in workspace.graph.packages() {
        let package_ids = release_package_ids(package)?;
        let package_is_claimed = package_ids.iter().any(|id| claimed.contains_key(id));
        if package_is_claimed {
            let changelog = package.changelog.as_ref().ok_or(GraphError::ReleaseIntentStale)?;
            if !changed_paths.contains(changelog.to_string_lossy().as_ref()) {
                return Err(GraphError::ReleaseIntentStale);
            }
        }
        for (manifest, id) in package.canonical_manifests().zip(package_ids) {
            let path = manifest.path.to_string_lossy();
            let before = &before_versions[manifest_index];
            let after = &after_versions[manifest_index];
            manifest_index += 1;
            let changed_version = before != after;
            match claimed.get(&id) {
                Some(target_version) => {
                    if !changed_version || after != target_version || !changed_paths.contains(path.as_ref()) {
                        return Err(GraphError::ReleaseIntentStale);
                    }
                    observed.insert(id);
                }
                None => {
                    if changed_version {
                        // A changed version the committed decision never
                        // claimed is not this commit's authority -- fail
                        // closed rather than trust a partial match.
                        return Err(GraphError::ReleaseIntentStale);
                    }
                }
            }
        }
    }
    if observed.len() != claimed.len() {
        return Err(GraphError::ReleaseIntentStale);
    }

    Ok(decision)
}

fn git_stdout<R: CommandRunner>(runner: &R, root: &std::path::Path, args: &[&str]) -> Result<String, GraphError> {
    let output = runner.run("git", args, root)?;
    if !output.success() {
        return Err(GraphError::ReleaseIntentStale);
    }
    Ok(output.stdout_trimmed().to_string())
}

fn parse_name_status(output: &str) -> Result<Vec<(String, String)>, GraphError> {
    output
        .lines()
        .map(|line| {
            let (status, path) = line.split_once('\t').ok_or(GraphError::ReleaseIntentStale)?;
            if !matches!(status, "A" | "M" | "D") || path.is_empty() || path.contains('\0') {
                return Err(GraphError::ReleaseIntentStale);
            }
            Ok((status.to_string(), path.to_string()))
        })
        .collect()
}

fn git_file<R: CommandRunner>(
    runner: &R,
    root: &std::path::Path,
    commit: &str,
    path: &str,
) -> Result<String, GraphError> {
    let object = format!("{commit}:{path}");
    git_stdout(runner, root, &["show", &object])
}

/// Extracts a manifest's declared version from its already-fetched raw
/// source text. Shared by the batched `git cat-file --batch` path in
/// [`derive_release_commit_decision`] (via [`resolve_batch_versions`]) and
/// this module's own tests (via the single-object `manifest_version_at`
/// helper they define), so both use identical parsing rules.
fn parse_manifest_version(source: &str, path: &str, ecosystem: Ecosystem) -> Result<Version, GraphError> {
    let format = ecosystem
        .canonical_manifest_format()
        .ok_or(GraphError::ReleaseIntentStale)?;
    // A malformed blob, a version-less manifest (e.g. a Cargo workspace
    // root with no [package] table, or a `dynamic = ["version"]` PEP 621
    // package), or a Cargo `version.workspace = true` inheritance (which
    // `read_identity` deliberately never resolves, having no workspace
    // context for a historical git blob) all fail closed here rather than
    // panicking or fabricating a version.
    let version = callisto_manifests::read_identity(format, source, std::path::Path::new(path))
        .ok()
        .and_then(|identity| match identity.version {
            Some(callisto_manifests::VersionSource::Literal(v)) => Some(v),
            _ => None,
        })
        .ok_or(GraphError::ReleaseIntentStale)?;
    Version::parse(&version, ecosystem.version_grammar()).map_err(|_error| GraphError::ReleaseIntentStale)
}

/// One object's content as reported by `git cat-file --batch`, for a single
/// requested `<commit>:<path>`.
enum BatchBlob {
    /// The path resolved to a blob at the queried commit; this is its raw
    /// content.
    Blob(String),
    /// The path did not exist at the queried commit -- e.g. a package's
    /// manifest that was only added later, so it has no blob yet at an
    /// earlier `parent` commit.
    Missing,
}

/// Issues exactly one `git cat-file --batch` invocation requesting every
/// path in `paths` as `<commit>:<path>`, and returns each result in the
/// same order as `paths`.
///
/// `git cat-file --batch` reads one object identifier per line from stdin
/// and, for each, writes to stdout either:
///
/// ```text
/// <sha> SP <type> SP <size> LF
/// <content: exactly `size` bytes> LF
/// ```
///
/// or, if the object doesn't exist:
///
/// ```text
/// <object> SP missing LF
/// ```
///
/// (git also reports `<object> SP ambiguous LF` for a ref that resolves to
/// more than one object; `<commit>:<path>` syntax should never be
/// ambiguous, but it's checked for explicitly below rather than silently
/// falling through to the blob-header parse.)
///
/// This lets every canonical manifest's content at one commit be fetched in
/// a single subprocess round trip instead of one `git show` per manifest.
///
/// Parsing here is deliberately defensive rather than permissive: this
/// feeds [`derive_release_commit_decision`]'s trust-boundary comparison, so
/// any framing ambiguity -- an unrecognized header shape, a declared `size`
/// that doesn't fit the remaining output, a missing separator byte right
/// after the declared content length, a non-`blob` object type, or a
/// leftover/truncated tail once every requested path has been accounted
/// for -- fails the whole batch closed rather than guessing. Each expected
/// response is also matched against its exact requested object string
/// (rather than a generic "ends with ` missing`" pattern), so a manifest
/// path that happened to contain a space or colon can't be misread as a
/// different response shape. Silently misattributing one manifest's
/// content to a different path because an undetected desync shifted every
/// later offset would be a far worse failure mode than an outright
/// rejection.
fn batch_manifest_blobs_at<R: CommandRunner>(
    runner: &R,
    root: &std::path::Path,
    commit: &str,
    paths: &[&str],
) -> Result<Vec<BatchBlob>, GraphError> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }

    let mut stdin = String::new();
    let objects: Vec<String> = paths
        .iter()
        .map(|path| {
            let object = format!("{commit}:{path}");
            stdin.push_str(&object);
            stdin.push('\n');
            object
        })
        .collect();

    let output = runner
        .run_with_stdin("git", &["cat-file", "--batch"], root, stdin.as_bytes())
        .map_err(|_error| GraphError::ReleaseIntentStale)?;
    if !output.success() {
        return Err(GraphError::ReleaseIntentStale);
    }

    let mut rest = output.stdout.as_str();
    let mut results = Vec::with_capacity(objects.len());
    for object in &objects {
        let (header, after_header) = rest.split_once('\n').ok_or(GraphError::ReleaseIntentStale)?;

        if header == format!("{object} missing") {
            results.push(BatchBlob::Missing);
            rest = after_header;
            continue;
        }
        if header == format!("{object} ambiguous") {
            return Err(GraphError::ReleaseIntentStale);
        }

        let mut fields = header.split(' ');
        let sha = fields.next().ok_or(GraphError::ReleaseIntentStale)?;
        let object_type = fields.next().ok_or(GraphError::ReleaseIntentStale)?;
        let size_field = fields.next().ok_or(GraphError::ReleaseIntentStale)?;
        if fields.next().is_some() {
            // A well-formed header has exactly three space-separated
            // fields; anything else is not a shape this parser understands.
            return Err(GraphError::ReleaseIntentStale);
        }
        if sha.is_empty() || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(GraphError::ReleaseIntentStale);
        }
        if object_type != "blob" {
            // Every path requested here is a canonical manifest file, so a
            // resolved object must be a blob; a tree/commit/tag response
            // means the path resolved to something else entirely.
            return Err(GraphError::ReleaseIntentStale);
        }
        let size: usize = size_field.parse().map_err(|_error| GraphError::ReleaseIntentStale)?;

        if size > after_header.len() {
            return Err(GraphError::ReleaseIntentStale);
        }
        // `.get(..size)` (byte-indexed) rather than raw slicing: if `size`
        // doesn't land on a UTF-8 char boundary this fails closed instead
        // of panicking.
        let content = after_header.get(..size).ok_or(GraphError::ReleaseIntentStale)?;
        let remainder = &after_header[size..];
        // The protocol always emits exactly one LF right after the
        // content, separate from the content itself. Requiring it here
        // catches a desynced `size` (from an earlier, unexpectedly
        // reencoded entry) as an explicit error instead of silently
        // parsing the wrong bytes as this entry's content.
        if remainder.as_bytes().first() != Some(&b'\n') {
            return Err(GraphError::ReleaseIntentStale);
        }
        results.push(BatchBlob::Blob(content.to_string()));
        rest = &remainder[1..];
    }

    if !rest.is_empty() {
        // More output than the requested objects account for -- either an
        // extra unexpected reply or a framing desync earlier in the
        // stream. Either way, fail closed rather than ignore it.
        return Err(GraphError::ReleaseIntentStale);
    }

    Ok(results)
}

/// Converts each batched blob-fetch result into the [`Version`] the
/// equivalent single-object `git show` fetch would have produced, using
/// [`parse_manifest_version`]'s parsing rules. A path missing from the
/// queried commit fails closed exactly as a `git show` that couldn't
/// resolve the path would have.
fn resolve_batch_versions(blobs: Vec<BatchBlob>, queries: &[(String, Ecosystem)]) -> Result<Vec<Version>, GraphError> {
    blobs
        .into_iter()
        .zip(queries.iter())
        .map(|(blob, (path, ecosystem))| match blob {
            BatchBlob::Blob(source) => parse_manifest_version(&source, path, *ecosystem),
            BatchBlob::Missing => Err(GraphError::ReleaseIntentStale),
        })
        .collect()
}

fn reason_from_bump(
    reason: Option<&BumpReason>,
    package_ids: &std::collections::BTreeMap<callisto_model::PackageId, Vec<ReleasePackageId>>,
) -> Result<ReleaseInclusionReason, GraphError> {
    match reason {
        Some(BumpReason::Changeset { .. }) | None => Ok(ReleaseInclusionReason::Changeset),
        Some(BumpReason::Inference { .. }) => Ok(ReleaseInclusionReason::Inference),
        Some(BumpReason::LinkedGroupUnion { group }) => Ok(ReleaseInclusionReason::LinkedGroup {
            group_id: group.to_string(),
        }),
        Some(BumpReason::FixedGroupUnion { group } | BumpReason::NewGroupMember { group }) => {
            Ok(ReleaseInclusionReason::FixedGroup {
                group_id: group.to_string(),
            })
        }
        Some(BumpReason::PreRelease { tag }) => Ok(ReleaseInclusionReason::PreReleasePolicy { policy_id: tag.clone() }),
        Some(BumpReason::Cascade { via, dep_kind, .. }) => {
            let source = package_ids
                .get(via)
                .and_then(|ids| (ids.len() == 1).then(|| ids[0].clone()))
                .ok_or(GraphError::ReleaseIntentStale)?;
            Ok(ReleaseInclusionReason::Cascade {
                from: source,
                edge_kind: format!("{dep_kind:?}"),
            })
        }
        Some(BumpReason::PeerEscalation { via, .. }) => {
            let source = package_ids
                .get(via)
                .and_then(|ids| (ids.len() == 1).then(|| ids[0].clone()))
                .ok_or(GraphError::ReleaseIntentStale)?;
            Ok(ReleaseInclusionReason::Cascade {
                from: source,
                edge_kind: "peer".to_string(),
            })
        }
        Some(_) => Err(GraphError::ReleaseIntentStale),
    }
}

#[cfg(test)]
mod tests {
    use callisto_model::{Ecosystem, Version};

    use super::*;

    #[test]
    fn decision_is_canonical_and_ecosystem_qualified() {
        let cargo = ReleasePackageId::new(Ecosystem::Cargo, "demo").unwrap();
        let npm = ReleasePackageId::new(Ecosystem::Npm, "demo").unwrap();
        let first = ReleaseDecisionV1::new(vec![
            ReleaseDecisionEntry {
                package: npm,
                target_version: Version::semver(1, 0, 0),
                reasons: vec![ReleaseInclusionReason::ExplicitSelection],
            },
            ReleaseDecisionEntry {
                package: cargo,
                target_version: Version::semver(1, 0, 0),
                reasons: vec![ReleaseInclusionReason::ExplicitSelection],
            },
        ])
        .unwrap();
        assert_eq!(first.entries[0].package.to_string(), "cargo/demo");
        assert_ne!(first.entries[0].package, first.entries[1].package);
    }

    #[test]
    fn release_commit_delta_parser_accepts_only_unambiguous_path_statuses() {
        assert_eq!(
            parse_name_status("M\tCargo.toml\nD\t.changeset/release.md\n").unwrap(),
            vec![
                ("M".to_string(), "Cargo.toml".to_string()),
                ("D".to_string(), ".changeset/release.md".to_string()),
            ]
        );
        assert!(parse_name_status("R100\told\tnew\n").is_err());
        assert!(parse_name_status("M Cargo.toml\n").is_err());
    }

    struct FixedBlobRunner(&'static str);

    impl CommandRunner for FixedBlobRunner {
        fn run(
            &self,
            _program: &str,
            _args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            Ok(callisto_model::CommandOutput {
                exit_code: Some(0),
                stdout: self.0.to_string(),
                stderr: String::new(),
            })
        }
    }

    /// Test-only: single-object equivalent of the batched
    /// `git cat-file --batch` path production code now uses (see
    /// `batch_manifest_blobs_at`/`resolve_batch_versions`), kept only so
    /// the two tests below can exercise `parse_manifest_version`'s parsing
    /// rules through a `CommandRunner` + single `git show` fetch, matching
    /// how they were originally written.
    fn manifest_version_at<R: CommandRunner>(
        runner: &R,
        root: &std::path::Path,
        commit: &str,
        path: &str,
        ecosystem: Ecosystem,
    ) -> Result<Version, GraphError> {
        let source = git_file(runner, root, commit, path)?;
        parse_manifest_version(&source, path, ecosystem)
    }

    // toml_edit's `Index` impl panics on a missing table ("index not found")
    // rather than returning None -- manifest_version_at previously used it
    // directly (`document["project"]["version"]`), so a Poetry-only
    // pyproject.toml, a PEP 621 `dynamic = ["version"]` package, or a
    // `Cargo.toml` with no `[package]` table would abort release-commit
    // verification with a panic instead of failing closed.
    #[test]
    fn manifest_version_at_fails_closed_instead_of_panicking_on_missing_tables() {
        let cases: &[(&str, Ecosystem)] = &[
            ("[project]\nname = \"x\"\ndynamic = [\"version\"]\n", Ecosystem::Pypi),
            ("[workspace]\nmembers = [\"a\"]\n", Ecosystem::Cargo),
        ];
        for (source, ecosystem) in cases {
            let runner = FixedBlobRunner(source);
            let result = manifest_version_at(&runner, std::path::Path::new("."), "deadbeef", "Cargo.toml", *ecosystem);
            assert!(
                matches!(result, Err(GraphError::ReleaseIntentStale)),
                "expected a clean ReleaseIntentStale rejection (not a panic) for {ecosystem:?}, got {result:?}"
            );
        }
    }

    #[test]
    fn manifest_version_at_still_resolves_poetry_fallback_and_normal_cargo() {
        let poetry = FixedBlobRunner("[tool.poetry]\nname = \"x\"\nversion = \"1.0.0\"\n");
        assert_eq!(
            manifest_version_at(
                &poetry,
                std::path::Path::new("."),
                "deadbeef",
                "pyproject.toml",
                Ecosystem::Pypi
            )
            .unwrap(),
            Version::parse("1.0.0", Ecosystem::Pypi.version_grammar()).unwrap()
        );

        let cargo = FixedBlobRunner("[package]\nname = \"x\"\nversion = \"1.2.3\"\n");
        assert_eq!(
            manifest_version_at(
                &cargo,
                std::path::Path::new("."),
                "deadbeef",
                "Cargo.toml",
                Ecosystem::Cargo
            )
            .unwrap(),
            Version::semver(1, 2, 3)
        );
    }

    /// A minimal fake [`DependencyResolver`] exposing a fixed set of
    /// packages, each with one canonical Cargo manifest and a changelog --
    /// just enough shape for [`derive_release_commit_decision`] to walk.
    struct FixedManifestGraph {
        packages: Vec<Package>,
    }

    impl DependencyResolver for FixedManifestGraph {
        fn packages(&self) -> impl Iterator<Item = &Package> {
            self.packages.iter()
        }

        fn dependencies_of(&self, _id: &callisto_model::PackageId) -> impl Iterator<Item = &callisto_model::DepEdge> {
            std::iter::empty()
        }

        fn dependents_of(&self, _id: &callisto_model::PackageId) -> impl Iterator<Item = &callisto_model::DepEdge> {
            std::iter::empty()
        }
    }

    fn cargo_package(name: &str) -> Package {
        Package {
            id: callisto_model::PackageId::parse(&format!("cargo:{name}")).unwrap(),
            manifests: vec![callisto_model::ManifestDecl::new(
                format!("{name}/Cargo.toml"),
                callisto_model::ManifestRole::Canonical,
                callisto_model::ManifestFormat::CargoToml,
            )
            .unwrap()],
            changelog: Some(std::path::PathBuf::from(format!("{name}/CHANGELOG.md"))),
            release_trigger: callisto_model::ReleaseTrigger::Changeset,
            publish_to: Vec::new(),
            tag_template: None,
        }
    }

    fn cargo_manifest_source(name: &str, version: &str) -> String {
        format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\n")
    }

    /// Records every `run`/`run_with_stdin` invocation and answers each the
    /// way a real `git` would for this test's fixed fixture, so
    /// `derive_release_commit_decision` can be driven entirely in-memory.
    /// `batch_calls` is the assertion this test cares about: it must land
    /// on exactly 2 (one `cat-file --batch` per commit) no matter how many
    /// canonical manifests `FixedManifestGraph` declares, proving the fix
    /// replaced 2*N `git show` spawns with 2 total, not N-scaled batches.
    struct CountingBatchRunner {
        release_commit: String,
        parent: String,
        decision_json: String,
        name_status: String,
        /// (commit, path) -> blob content; a path/commit pair with no entry
        /// here is answered as `missing`, matching real `git cat-file
        /// --batch` behavior for a path that doesn't exist at that commit.
        blobs: std::collections::BTreeMap<(String, String), String>,
        run_calls: std::sync::Mutex<Vec<(String, Vec<String>)>>,
        batch_calls: std::sync::Mutex<u32>,
    }

    impl CommandRunner for CountingBatchRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &std::path::Path,
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            self.run_calls
                .lock()
                .unwrap()
                .push((program.to_string(), args.iter().map(|s| s.to_string()).collect()));

            let ok = |stdout: String| {
                Ok(callisto_model::CommandOutput {
                    exit_code: Some(0),
                    stdout,
                    stderr: String::new(),
                })
            };
            let rev_parse_parent = format!("{}^", self.release_commit);
            let decision_object = format!("{}:.callisto/release-decision.json", self.release_commit);
            if args.first().copied() == Some("rev-parse") && args.get(1).copied() == Some("HEAD") {
                return ok(self.release_commit.clone());
            }
            if args.first().copied() == Some("rev-parse") && args.get(1).copied() == Some(rev_parse_parent.as_str()) {
                return ok(self.parent.clone());
            }
            if args.first().copied() == Some("diff-tree") {
                return ok(self.name_status.clone());
            }
            if args.first().copied() == Some("show") && args.get(1).copied() == Some(decision_object.as_str()) {
                return ok(self.decision_json.clone());
            }
            panic!("unexpected `{program} {args:?}` in CountingBatchRunner::run");
        }

        fn run_with_stdin(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &std::path::Path,
            stdin: &[u8],
        ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
            self.run_calls
                .lock()
                .unwrap()
                .push((program.to_string(), args.iter().map(|s| s.to_string()).collect()));
            assert_eq!(program, "git");
            assert_eq!(args, ["cat-file", "--batch"]);
            *self.batch_calls.lock().unwrap() += 1;

            let requested = std::str::from_utf8(stdin).expect("test stdin is always valid UTF-8");
            let mut stdout = String::new();
            for object in requested.lines() {
                let (commit, path) = object.split_once(':').expect("object identifier must be commit:path");
                match self.blobs.get(&(commit.to_string(), path.to_string())) {
                    Some(content) => {
                        stdout.push_str(&format!("{} blob {}\n{}\n", "0".repeat(40), content.len(), content));
                    }
                    None => {
                        stdout.push_str(&format!("{object} missing\n"));
                    }
                }
            }
            Ok(callisto_model::CommandOutput {
                exit_code: Some(0),
                stdout,
                stderr: String::new(),
            })
        }
    }

    /// AC: for a release commit touching multiple canonical manifests,
    /// `derive_release_commit_decision` issues exactly 2 `git cat-file
    /// --batch` invocations (one per commit) -- not 2*N separate `git show`
    /// spawns, one per manifest per commit, as the pre-fix implementation
    /// did.
    #[test]
    fn derive_release_commit_decision_issues_exactly_two_batch_invocations_for_multiple_manifests() {
        let release_commit_sha = "a".repeat(40);
        let parent_sha = "b".repeat(40);

        let packages = vec![cargo_package("pkg-a"), cargo_package("pkg-b"), cargo_package("pkg-c")];

        let bumps = [
            ("pkg-a", "1.0.0", "1.1.0"),
            ("pkg-b", "2.0.0", "2.1.0"),
            ("pkg-c", "3.0.0", "3.1.0"),
        ];

        let mut blobs = std::collections::BTreeMap::new();
        let mut entries = Vec::new();
        for (name, before, after) in bumps {
            let path = format!("{name}/Cargo.toml");
            blobs.insert((parent_sha.clone(), path.clone()), cargo_manifest_source(name, before));
            blobs.insert((release_commit_sha.clone(), path), cargo_manifest_source(name, after));
            let target = Version::parse(after, Ecosystem::Cargo.version_grammar()).unwrap();
            entries.push(ReleaseDecisionEntry {
                package: ReleasePackageId::new(Ecosystem::Cargo, name).unwrap(),
                target_version: target,
                reasons: vec![ReleaseInclusionReason::Changeset],
            });
        }
        let decision = ReleaseDecisionV1::new(entries).unwrap();
        let decision_json = serde_json::to_string(&decision).unwrap();

        let name_status = [
            "A\t.callisto/release-decision.json".to_string(),
            "D\t.changeset/multi-pkg-minor.md".to_string(),
            "M\tpkg-a/Cargo.toml".to_string(),
            "M\tpkg-a/CHANGELOG.md".to_string(),
            "M\tpkg-b/Cargo.toml".to_string(),
            "M\tpkg-b/CHANGELOG.md".to_string(),
            "M\tpkg-c/Cargo.toml".to_string(),
            "M\tpkg-c/CHANGELOG.md".to_string(),
        ]
        .join("\n");

        let runner = CountingBatchRunner {
            release_commit: release_commit_sha.clone(),
            parent: parent_sha,
            decision_json,
            name_status,
            blobs,
            run_calls: std::sync::Mutex::new(Vec::new()),
            batch_calls: std::sync::Mutex::new(0),
        };

        let config = crate::config::load(std::path::Path::new("/nonexistent-test-root")).unwrap();
        let graph = FixedManifestGraph { packages };
        let workspace = Workspace {
            root: std::path::PathBuf::from("/nonexistent-test-root"),
            config,
            graph,
            tags: std::cell::OnceCell::new(),
            git: std::cell::OnceCell::new(),
            runner: &runner,
            manifest_cache: Default::default(),
            identity: crate::IdentityIndex::default(),
        };

        let release_commit = CommitSha::parse(&release_commit_sha).unwrap();
        let decision_path = std::path::Path::new(".callisto/release-decision.json");
        let result = derive_release_commit_decision(&workspace, &release_commit, decision_path);

        assert!(result.is_ok(), "expected Ok, got {result:?}");
        assert_eq!(
            *runner.batch_calls.lock().unwrap(),
            2,
            "expected exactly 2 `git cat-file --batch` invocations (one per commit) for 3 \
             canonical manifests, not one per manifest per commit"
        );
    }
}
