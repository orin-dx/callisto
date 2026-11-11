use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use callisto_model::vcs::{GitAccess, VcsError};
use callisto_model::{Package, PackageId};

use crate::config::GroupTable;
use crate::error::GraphError;
use crate::tags::TagIndex;

pub fn package_paths(pkg: &Package) -> Vec<PathBuf> {
    let mut set = std::collections::BTreeSet::new();
    for m in &pkg.manifests {
        if let Some(parent) = m.path.parent() {
            if parent.as_os_str().is_empty() {
                set.insert(PathBuf::from("."));
            } else {
                set.insert(parent.to_path_buf());
            }
        }
    }
    set.into_iter().collect()
}

/// Whether each of `packages` changed since its last tag: a directory of the package holds different files than it did
/// at the tag, or its tracked files have uncommitted changes. An untagged package counts as changed. A change that was
/// made and then reverted leaves the package as it was released, so it is not a change.
///
/// Directories are compared by git tree id, all asked for in one `git cat-file --batch-check`, so the cost does not
/// grow with the number of commits. Uncommitted changes come from one `git diff`.
pub fn changed_since_last_tag(
    packages: &[&Package],
    tags: &TagIndex,
    git: &GitAccess<'_>,
) -> Result<BTreeMap<PackageId, bool>, GraphError> {
    let mut changed = BTreeMap::new();
    let mut tagged = Vec::new();
    for pkg in packages {
        match tags.last_tag(&pkg.id) {
            Some(last) => tagged.push((*pkg, last.name.as_str())),
            None => {
                changed.insert(pkg.id.clone(), true);
            }
        }
    }
    if tagged.is_empty() {
        return Ok(changed);
    }

    let specs: Vec<Vec<PathBuf>> = tagged.iter().map(|(pkg, _)| package_paths(pkg)).collect();
    let mut differs = vec![false; tagged.len()];
    // Each package directory is asked for twice, at its tag and at `HEAD`; `pair_owners` says whose pair it is.
    let mut objects = Vec::new();
    let mut pair_owners = Vec::new();
    for (index, ((_, tag), directories)) in tagged.iter().zip(&specs).enumerate() {
        let Some(commit) = tags.commit_of(git, tag)? else {
            // A tag that does not end at a commit cannot be compared, so the package is proposed for release.
            differs[index] = true;
            continue;
        };
        for directory in directories {
            // `./` anchors the path at the working directory; a bare `rev:path` resolves from the repository top.
            let directory = if directory.as_path() == Path::new(".") {
                "./".to_owned()
            } else {
                format!("./{}", directory.to_string_lossy())
            };
            objects.push(format!("{}:{directory}", commit.as_str()));
            objects.push(format!("HEAD:{directory}"));
            pair_owners.push(index);
        }
    }
    let ids = git.object_ids(&objects)?;
    for (pair, &owner) in pair_owners.iter().enumerate() {
        if ids[2 * pair] != ids[2 * pair + 1] {
            differs[owner] = true;
        }
    }

    // Uncommitted changes count for every package they touch; a diff that could not run counts as changed.
    let uncommitted = match git.paths_differing_from("HEAD") {
        Ok(paths) => Some(paths),
        Err(VcsError::Command(e)) => return Err(VcsError::Command(e).into()),
        Err(_) => None,
    };
    let owners = SpecIndex::new(&specs);
    let mut dirty = vec![uncommitted.is_none(); tagged.len()];
    for path in uncommitted.iter().flatten() {
        owners.owners_of(path, |package| dirty[package] = true);
    }
    for (index, (pkg, _)) in tagged.iter().enumerate() {
        changed.insert(pkg.id.clone(), differs[index] || dirty[index]);
    }
    Ok(changed)
}

/// Paths whose edits do not change what ships: tests, benches, examples, snapshots and Markdown.
pub const DEFAULT_IGNORED_PATHS: &[&str] = &["tests/", "benches/", "examples/", "*.snap", "*.md", "CHANGELOG.md"];

/// Files under each package's directories that differ between its last tag and `HEAD` or the working tree, minus the
/// `ignore` pathspecs (a trailing `/` names a directory at any depth, anything else a file glob). An untagged package, or
/// one whose tag is not a commit, yields all its tracked files. One git invocation per distinct tag commit, plus one per
/// untagged package.
pub fn changed_source_files_since_last_tag(
    packages: &[&Package],
    tags: &TagIndex,
    git: &GitAccess<'_>,
    ignore: &[&str],
) -> Result<BTreeMap<PackageId, Vec<PathBuf>>, GraphError> {
    changed_source_files_in_batches(packages, tags, git, ignore, PATHSPEC_BUDGET)
}

/// The most pathspec bytes one `git diff` gets, so a very large workspace stays under a command-line length limit
/// (32 KB on Windows) by splitting into a few diffs instead of one.
const PATHSPEC_BUDGET: usize = 24_000;

fn changed_source_files_in_batches(
    packages: &[&Package],
    tags: &TagIndex,
    git: &GitAccess<'_>,
    ignore: &[&str],
    budget: usize,
) -> Result<BTreeMap<PackageId, Vec<PathBuf>>, GraphError> {
    let excludes: Vec<String> = ignore.iter().map(|pattern| exclude_pathspec(pattern)).collect();
    let mut result = BTreeMap::new();
    let mut by_base: BTreeMap<String, Vec<(&Package, Vec<PathBuf>)>> = BTreeMap::new();
    for pkg in packages {
        let base = match tags.last_tag(&pkg.id) {
            Some(last) => tags.commit_of(git, last.name.as_str())?,
            None => None,
        };
        let directories = package_paths(pkg);
        match base {
            // With no directory the pathspec would be empty and match everything, so it is not batched.
            Some(commit) if !directories.is_empty() => {
                by_base
                    .entry(commit.as_str().to_owned())
                    .or_default()
                    .push((pkg, directories));
            }
            Some(commit) => {
                let files = list_files(git, &diff_command(commit.as_str()), &directories, &excludes)?;
                result.insert(pkg.id.clone(), files.into_iter().collect());
            }
            None => {
                let files = list_files(git, &["ls-files", "-z"], &directories, &excludes)?;
                result.insert(pkg.id.clone(), files.into_iter().collect());
            }
        }
    }
    for (commit, group) in &by_base {
        let specs: Vec<Vec<PathBuf>> = group.iter().map(|(_, directories)| directories.clone()).collect();
        let all: Vec<PathBuf> = specs.iter().flatten().cloned().collect();
        let listed = list_in_chunks(git, &diff_command(commit), &all, &excludes, budget)?;
        // A file belongs to every package whose directory contains it, as it did when each package had its own diff.
        let index = SpecIndex::new(&specs);
        let mut owned: Vec<BTreeSet<PathBuf>> = vec![BTreeSet::new(); group.len()];
        for path in listed {
            index.owners_of(&path, |package| {
                owned[package].insert(path.clone());
            });
        }
        for ((pkg, _), files) in group.iter().zip(owned) {
            result.insert(pkg.id.clone(), files.into_iter().collect());
        }
    }
    Ok(result)
}

fn diff_command(commit: &str) -> [&str; 6] {
    ["diff", "--no-renames", "--relative", "--name-only", "-z", commit]
}

/// `list_files` over `directories`, split so no command carries more than `budget` bytes of pathspec.
fn list_in_chunks(
    git: &GitAccess<'_>,
    command: &[&str],
    directories: &[PathBuf],
    excludes: &[String],
    budget: usize,
) -> Result<BTreeSet<PathBuf>, GraphError> {
    let fixed: usize = excludes.iter().map(|exclude| exclude.len() + 1).sum();
    let mut listed = BTreeSet::new();
    let mut start = 0;
    while start < directories.len() {
        let mut end = start;
        let mut bytes = fixed;
        while end < directories.len() && (end == start || bytes + directories[end].as_os_str().len() < budget) {
            bytes += directories[end].as_os_str().len() + 1;
            end += 1;
        }
        listed.extend(list_files(git, command, &directories[start..end], excludes)?);
        start = end;
    }
    Ok(listed)
}

/// The paths `git <command> -- <directories> <excludes>` prints, which is NUL-separated.
fn list_files(
    git: &GitAccess<'_>,
    command: &[&str],
    directories: &[PathBuf],
    excludes: &[String],
) -> Result<BTreeSet<PathBuf>, GraphError> {
    let mut args: Vec<String> = command.iter().map(|arg| (*arg).to_owned()).collect();
    args.push("--".to_owned());
    args.extend(directories.iter().map(|d| d.to_string_lossy().into_owned()));
    args.extend(excludes.iter().cloned());
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = git.runner().run("git", &arg_refs, git.root()).map_err(VcsError::from)?;
    if !output.success() {
        return Err(VcsError::Git(format!(
            "`git {}` failed in `{}`: {}",
            args[0],
            git.root().display(),
            output.redacted_stderr()
        ))
        .into());
    }
    Ok(output
        .stdout
        .split('\0')
        .map(|path| path.trim_start_matches('\n'))
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect())
}

fn exclude_pathspec(pattern: &str) -> String {
    match pattern.strip_suffix('/') {
        Some(directory) => format!(":(exclude,glob)**/{directory}/**"),
        None => format!(":(exclude,glob)**/{pattern}"),
    }
}

/// Whether `id` is released with `covered`: it is covered itself, or a member of its fixed or linked group is, since a
/// group moves together.
pub fn is_covered(id: &PackageId, covered: &BTreeSet<PackageId>, groups: &GroupTable) -> bool {
    covered.contains(id)
        || [(&groups.fixed_of, &groups.fixed), (&groups.linked_of, &groups.linked)]
            .into_iter()
            .filter_map(|(of, defs)| of.get(id).and_then(|name| defs.get(name)))
            .any(|group| group.package_members().any(|member| covered.contains(member)))
}

/// Packages with a changed source file that nothing releases: not [covered](is_covered).
pub fn uncovered_changes(
    changed: &BTreeMap<PackageId, Vec<PathBuf>>,
    covered: &BTreeSet<PackageId>,
    groups: &GroupTable,
) -> Vec<PackageId> {
    changed
        .iter()
        .filter(|(id, files)| !files.is_empty() && !is_covered(id, covered, groups))
        .map(|(id, _)| id.clone())
        .collect()
}

/// The changed source files of the packages in `packages` that nothing releases, without a git process per package.
/// Covered packages are skipped outright, whether or not they changed; the rest are told apart by tree id in a fixed
/// number of processes, and only a package that really changed pays for its own file listing.
pub fn changed_source_files_of_uncovered(
    packages: &[&Package],
    tags: &TagIndex,
    git: &GitAccess<'_>,
    covered: &BTreeSet<PackageId>,
    groups: &GroupTable,
    ignore: &[&str],
) -> Result<BTreeMap<PackageId, Vec<PathBuf>>, GraphError> {
    let candidates: Vec<&Package> = packages
        .iter()
        .copied()
        .filter(|package| !is_covered(&package.id, covered, groups))
        .collect();
    let touched = changed_since_last_tag(&candidates, tags, git)?;
    let changed: Vec<&Package> = candidates
        .into_iter()
        .filter(|package| touched.get(&package.id).copied().unwrap_or(false))
        .collect();
    changed_source_files_since_last_tag(&changed, tags, git, ignore)
}

/// Which packages a path belongs to: those with a spec that is an ancestor directory of it, and those with the `.`
/// spec, which every path falls under.
struct SpecIndex<'a> {
    by_directory: std::collections::HashMap<&'a Path, Vec<usize>>,
    everywhere: Vec<usize>,
}

impl<'a> SpecIndex<'a> {
    fn new(specs: &'a [Vec<PathBuf>]) -> Self {
        let mut by_directory: std::collections::HashMap<&Path, Vec<usize>> = std::collections::HashMap::new();
        let mut everywhere = Vec::new();
        for (package, package_specs) in specs.iter().enumerate() {
            for spec in package_specs {
                if spec.as_path() == Path::new(".") {
                    everywhere.push(package);
                } else {
                    by_directory.entry(spec.as_path()).or_default().push(package);
                }
            }
        }
        SpecIndex {
            by_directory,
            everywhere,
        }
    }

    fn owners_of(&self, path: &Path, mut found: impl FnMut(usize)) {
        for &package in &self.everywhere {
            found(package);
        }
        for directory in path.ancestors() {
            for &package in self.by_directory.get(directory).into_iter().flatten() {
                found(package);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use callisto_fixtures::git::{init_repo, run_git, GitRunner};
    use callisto_model::{
        CommandError, CommandOutput, CommandRunner, DepEdge, ManifestDecl, ManifestFormat, ManifestRole,
    };
    use std::sync::Mutex;

    use super::*;
    use crate::resolver::DependencyResolver;

    fn make_pkg(dir: &str) -> Package {
        let manifest_path = if dir == "." {
            "Cargo.toml".to_string()
        } else {
            format!("{dir}/Cargo.toml")
        };
        let name = if dir == "." { "root" } else { dir };
        Package {
            id: PackageId::parse(name).unwrap(),
            manifests: vec![
                ManifestDecl::new(manifest_path, ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap(),
            ],
            changelog: None,
            release_trigger: callisto_model::ReleaseTrigger::Changeset,
            publish_to: Vec::new(),
            tag_template: None,
        }
    }

    struct FixedGraph {
        pkgs: Vec<Package>,
    }

    impl DependencyResolver for FixedGraph {
        fn packages(&self) -> impl Iterator<Item = &Package> {
            self.pkgs.iter()
        }

        fn dependencies_of(&self, _id: &PackageId) -> impl Iterator<Item = &DepEdge> {
            std::iter::empty()
        }

        fn dependents_of(&self, _id: &PackageId) -> impl Iterator<Item = &DepEdge> {
            std::iter::empty()
        }
    }

    /// Records every `git` invocation and runs it for real.
    struct RecordingGit {
        calls: Mutex<Vec<String>>,
    }

    impl CommandRunner for RecordingGit {
        fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
            self.calls.lock().unwrap().push(args.join(" "));
            GitRunner.run(program, args, cwd)
        }

        fn run_with_stdin(
            &self,
            program: &str,
            args: &[&str],
            cwd: &Path,
            stdin: &[u8],
        ) -> Result<CommandOutput, CommandError> {
            self.calls.lock().unwrap().push(args.join(" "));
            GitRunner.run_with_stdin(program, args, cwd, stdin)
        }
    }

    fn write(root: &Path, path: &str, contents: &str) {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, contents).unwrap();
    }

    fn commit(root: &Path, path: &str, contents: &str, message: &str) {
        write(root, path, contents);
        run_git(root, &["add", "-A"]);
        run_git(root, &["commit", "-q", "-m", message]);
    }

    /// Repo with `pkg-a`, `pkg-b` and `pkg-c`, each tagged `<pkg>@1.0.0` at one commit.
    fn tagged_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        for pkg in ["pkg-a", "pkg-b", "pkg-c"] {
            write(root, &format!("{pkg}/Cargo.toml"), "[package]\n");
        }
        commit(root, "README.md", "r\n", "chore: init");
        for pkg in ["pkg-a", "pkg-b", "pkg-c"] {
            run_git(root, &["tag", "-a", "-m", "release", &format!("{pkg}@1.0.0")]);
        }
        dir
    }

    fn changed(root: &Path, pkgs: &[Package]) -> (BTreeMap<PackageId, bool>, Vec<String>) {
        let graph = FixedGraph { pkgs: pkgs.to_vec() };
        let cfg = crate::config::load(root).unwrap();
        let runner = RecordingGit {
            calls: Mutex::new(Vec::new()),
        };
        let git = GitAccess::new(root, &runner);
        let tags = TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();
        runner.calls.lock().unwrap().clear();
        let refs: Vec<&Package> = pkgs.iter().collect();
        let result = changed_since_last_tag(&refs, &tags, &git).unwrap();
        let calls = runner.calls.lock().unwrap().clone();
        (result, calls)
    }

    fn id(name: &str) -> PackageId {
        PackageId::parse(name).unwrap()
    }

    #[test]
    fn untagged_package_is_changed_without_running_git() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = make_pkg("pkg-a");
        let runner = RecordingGit {
            calls: Mutex::new(Vec::new()),
        };
        let git = GitAccess::new(dir.path(), &runner);

        let result = changed_since_last_tag(&[&pkg], &TagIndex::empty(), &git).unwrap();

        assert!(result[&id("pkg-a")]);
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    /// Only the package a commit changed differs from its tag. The tags' commits come from one `git for-each-ref`, the
    /// trees from one `git cat-file`, and the uncommitted changes from one `git diff`; no history is read.
    #[test]
    fn a_commit_changes_only_the_package_it_touched() {
        let dir = tagged_repo();
        let root = dir.path();
        commit(root, "pkg-a/src/lib.rs", "a\n", "feat: a");
        let pkgs = [make_pkg("pkg-a"), make_pkg("pkg-b"), make_pkg("pkg-c")];

        let (result, calls) = changed(root, &pkgs);

        assert!(result[&id("pkg-a")]);
        assert!(!result[&id("pkg-b")]);
        assert!(!result[&id("pkg-c")]);
        let count = |sub: &str| calls.iter().filter(|c| c.starts_with(sub)).count();
        assert_eq!(count("for-each-ref"), 1, "{calls:?}");
        assert_eq!(count("cat-file"), 1, "{calls:?}");
        assert_eq!(count("diff"), 1, "{calls:?}");
        assert_eq!(count("log"), 0, "{calls:?}");
        assert_eq!(count("rev-parse"), 0, "{calls:?}");
    }

    /// Object specs resolve from the repository top level, so a workspace below it must still see its own packages.
    #[test]
    fn a_workspace_below_the_repository_root_sees_committed_changes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let ws = repo.join("ws");
        init_repo(repo);
        write(&ws, "pkg-a/Cargo.toml", "[package]\n");
        commit(repo, "README.md", "r\n", "chore: init");
        run_git(repo, &["tag", "-a", "-m", "release", "pkg-a@0.1.0"]);
        commit(repo, "ws/pkg-a/src/lib.rs", "a\n", "feat: a");

        let (result, _) = changed(&ws, &[make_pkg("pkg-a")]);

        assert!(result[&id("pkg-a")]);
    }

    #[test]
    fn an_uncommitted_change_counts_as_changed() {
        let dir = tagged_repo();
        let root = dir.path();
        write(root, "pkg-b/Cargo.toml", "[package]\nname = \"b\"\n");
        let pkgs = [make_pkg("pkg-a"), make_pkg("pkg-b")];

        let (result, _) = changed(root, &pkgs);

        assert!(!result[&id("pkg-a")]);
        assert!(result[&id("pkg-b")]);
    }

    /// A change that a later commit reverted leaves the package as it was released, so it is not a change.
    #[test]
    fn a_reverted_change_is_not_a_change() {
        let dir = tagged_repo();
        let root = dir.path();
        commit(root, "pkg-a/x.txt", "x\n", "feat: x");
        run_git(root, &["rm", "-q", "pkg-a/x.txt"]);
        run_git(root, &["commit", "-q", "-m", "revert: x"]);
        let pkgs = [make_pkg("pkg-a")];

        let (result, _) = changed(root, &pkgs);

        assert!(!result[&id("pkg-a")]);
    }

    /// Moving a file out of a package changes it: renames count for both paths.
    #[test]
    fn a_rename_out_of_a_package_changes_it() {
        let dir = tagged_repo();
        let root = dir.path();
        run_git(root, &["mv", "pkg-a/Cargo.toml", "pkg-b/moved.toml"]);
        run_git(root, &["commit", "-q", "-m", "refactor: move"]);
        let pkgs = [make_pkg("pkg-a"), make_pkg("pkg-b"), make_pkg("pkg-c")];

        let (result, _) = changed(root, &pkgs);

        assert!(result[&id("pkg-a")]);
        assert!(result[&id("pkg-b")]);
        assert!(!result[&id("pkg-c")]);
    }

    /// A path that merely shares a prefix with a package is not in it.
    #[test]
    fn a_sibling_with_a_shared_prefix_does_not_change_the_package() {
        let dir = tagged_repo();
        let root = dir.path();
        commit(root, "pkg-ab/f.txt", "f\n", "feat: sibling");
        let pkgs = [make_pkg("pkg-a")];

        let (result, _) = changed(root, &pkgs);

        assert!(!result[&id("pkg-a")]);
    }

    /// Packages tagged at different commits are each compared with their own tag.
    #[test]
    fn packages_tagged_at_different_commits_use_their_own_tag() {
        let dir = tagged_repo();
        let root = dir.path();
        commit(root, "pkg-a/f.txt", "1\n", "feat: a before b's release");
        run_git(root, &["tag", "-a", "-m", "release", "pkg-b@1.1.0"]);
        let pkgs = [make_pkg("pkg-a"), make_pkg("pkg-b")];

        let (result, _) = changed(root, &pkgs);

        assert!(result[&id("pkg-a")]);
        assert!(!result[&id("pkg-b")]);
    }

    /// A package whose directory did not exist at its tag differs from it; one that never existed does not.
    #[test]
    fn a_directory_missing_at_the_tag_is_a_change_and_one_missing_everywhere_is_not() {
        let dir = tagged_repo();
        let root = dir.path();
        commit(root, "pkg-d/Cargo.toml", "[package]\n", "feat: add pkg-d");
        run_git(root, &["tag", "-a", "-m", "release", "pkg-d@1.0.0", "HEAD~1"]);
        run_git(root, &["tag", "-a", "-m", "release", "pkg-ghost@1.0.0", "HEAD~1"]);
        let pkgs = [make_pkg("pkg-d"), make_pkg("pkg-ghost")];

        let (result, _) = changed(root, &pkgs);

        assert!(result[&id("pkg-d")]);
        assert!(!result[&id("pkg-ghost")]);
    }

    /// A tag on a side branch, a merge, and a commit on each line: each package is compared with its own tag.
    #[test]
    fn packages_are_compared_with_their_tags_across_a_merge() {
        let dir = tagged_repo();
        let root = dir.path();
        run_git(root, &["switch", "-q", "-c", "side"]);
        commit(root, "pkg-b/side.txt", "s\n", "feat: b on the side");
        run_git(root, &["tag", "-a", "-m", "release", "pkg-c@1.1.0"]);
        run_git(root, &["switch", "-q", "-"]);
        commit(root, "pkg-a/main.txt", "m\n", "feat: a on main");
        run_git(root, &["merge", "-q", "--no-ff", "-m", "merge side", "side"]);
        commit(root, "pkg-c/later.txt", "l\n", "feat: c after the merge");
        run_git(root, &["tag", "-a", "-m", "release", "pkg-a@1.1.0"]);
        write(root, "pkg-b/Cargo.toml", "[package]\nname = \"b\"\n");
        let pkgs = [make_pkg("pkg-a"), make_pkg("pkg-b"), make_pkg("pkg-c")];

        let (result, _) = changed(root, &pkgs);

        assert!(!result[&id("pkg-a")], "a was tagged after the merge");
        assert!(
            result[&id("pkg-b")],
            "b changed on the side branch and in the working tree"
        );
        assert!(result[&id("pkg-c")], "c changed after the merge");
    }

    /// A package released from a branch that was never merged: `HEAD` lacks the branch's fix, so the package differs
    /// from its tag.
    #[test]
    fn a_tag_on_an_unmerged_branch_differs_from_head_by_the_branch_and_by_what_came_after() {
        let dir = tagged_repo();
        let root = dir.path();
        commit(root, "pkg-a/shared.txt", "s\n", "feat: a before the split");
        run_git(root, &["switch", "-q", "-c", "hotfix"]);
        commit(root, "pkg-b/fix.txt", "f\n", "fix: b on the hotfix branch");
        run_git(root, &["tag", "-a", "-m", "release", "pkg-b@1.0.1"]);
        run_git(root, &["switch", "-q", "-"]);
        commit(root, "pkg-c/later.txt", "l\n", "feat: c on the main line");
        let pkgs = [make_pkg("pkg-a"), make_pkg("pkg-b"), make_pkg("pkg-c")];

        let (result, _) = changed(root, &pkgs);

        assert!(result[&id("pkg-a")], "a changed before the split, after its own tag");
        assert!(result[&id("pkg-b")], "HEAD lacks the fix the hotfix tag has");
        assert!(result[&id("pkg-c")]);
    }

    /// The root-level package's `.` is the whole tree, so any commit changes it.
    #[test]
    fn root_package_is_changed_by_any_commit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        commit(root, "Cargo.toml", "[package]\n", "chore: init");
        run_git(root, &["tag", "-a", "-m", "release", "root@1.0.0"]);
        commit(root, "src/lib.rs", "x\n", "feat: x");
        let pkgs = [make_pkg(".")];

        let (result, _) = changed(root, &pkgs);

        assert!(result[&id("root")]);
    }

    fn sources(root: &Path, pkgs: &[Package], ignore: &[&str]) -> BTreeMap<PackageId, Vec<PathBuf>> {
        let graph = FixedGraph { pkgs: pkgs.to_vec() };
        let cfg = crate::config::load(root).unwrap();
        let runner = GitRunner;
        let git = GitAccess::new(root, &runner);
        let tags = TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();
        let refs: Vec<&Package> = pkgs.iter().collect();
        changed_source_files_since_last_tag(&refs, &tags, &git, ignore).unwrap()
    }

    fn files(map: &BTreeMap<PackageId, Vec<PathBuf>>, name: &str) -> Vec<String> {
        map[&id(name)]
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    /// A repo whose `count` packages `pkg-0`.. are each tagged `<pkg>@1.0.0`, plus the packages themselves.
    fn tagged_repo_of(count: usize) -> (tempfile::TempDir, Vec<Package>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        let names: Vec<String> = (0..count).map(|index| format!("pkg-{index}")).collect();
        for name in &names {
            write(root, &format!("{name}/Cargo.toml"), "[package]\n");
            write(root, &format!("{name}/src/lib.rs"), "\n");
        }
        commit(root, "README.md", "r\n", "chore: init");
        for name in &names {
            run_git(root, &["tag", "-a", "-m", "release", &format!("{name}@1.0.0")]);
        }
        let packages = names.iter().map(|name| make_pkg(name)).collect();
        (dir, packages)
    }

    /// The uncovered changed files of `pkgs` and how many git processes finding them started.
    fn uncovered_files(
        root: &Path,
        pkgs: &[Package],
        covered: &BTreeSet<PackageId>,
    ) -> (BTreeMap<PackageId, Vec<PathBuf>>, usize) {
        let graph = FixedGraph { pkgs: pkgs.to_vec() };
        let cfg = crate::config::load(root).unwrap();
        let runner = RecordingGit {
            calls: Mutex::new(Vec::new()),
        };
        let git = GitAccess::new(root, &runner);
        let tags = TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();
        let before = runner.calls.lock().unwrap().len();
        let refs: Vec<&Package> = pkgs.iter().collect();
        let files =
            changed_source_files_of_uncovered(&refs, &tags, &git, covered, &Default::default(), DEFAULT_IGNORED_PATHS)
                .unwrap();
        let used = runner.calls.lock().unwrap().len() - before;
        (files, used)
    }

    /// AGENTS.md: a change that adds a git process per package fails review. Unchanged packages must cost nothing extra.
    #[test]
    fn unchanged_packages_add_no_git_processes() {
        let (small_dir, small) = tagged_repo_of(3);
        let (large_dir, large) = tagged_repo_of(24);

        let (small_files, small_calls) = uncovered_files(small_dir.path(), &small, &BTreeSet::new());
        let (large_files, large_calls) = uncovered_files(large_dir.path(), &large, &BTreeSet::new());

        assert!(small_files.is_empty() && large_files.is_empty());
        assert_eq!(small_calls, large_calls, "the cost grew with the package count");
    }

    #[test]
    fn only_an_uncovered_package_that_changed_pays_for_a_file_listing() {
        let (dir, pkgs) = tagged_repo_of(6);
        let root = dir.path();
        commit(root, "pkg-2/src/lib.rs", "changed\n", "feat: two");
        commit(root, "pkg-4/src/lib.rs", "changed\n", "feat: four");
        let covered: BTreeSet<PackageId> = [id("pkg-4")].into_iter().collect();

        let (files, calls) = uncovered_files(root, &pkgs, &covered);
        let (none, base_calls) = uncovered_files(tagged_repo_of(6).0.path(), &pkgs, &covered);

        assert_eq!(files.keys().collect::<Vec<_>>(), [&id("pkg-2")]);
        assert!(none.is_empty());
        assert_eq!(
            calls,
            base_calls + 1,
            "one more process, for the one changed uncovered package"
        );
    }

    /// `changed_source_files_since_last_tag` over `pkgs`, and how many `git diff` processes it started.
    fn sources_counting_diffs(root: &Path, pkgs: &[Package]) -> (BTreeMap<PackageId, Vec<PathBuf>>, usize) {
        let graph = FixedGraph { pkgs: pkgs.to_vec() };
        let cfg = crate::config::load(root).unwrap();
        let runner = RecordingGit {
            calls: Mutex::new(Vec::new()),
        };
        let git = GitAccess::new(root, &runner);
        let tags = TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();
        let refs: Vec<&Package> = pkgs.iter().collect();
        let result = changed_source_files_since_last_tag(&refs, &tags, &git, DEFAULT_IGNORED_PATHS).unwrap();
        let diffs = runner
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.starts_with("diff "))
            .count();
        (result, diffs)
    }

    #[test]
    fn packages_sharing_a_base_commit_are_listed_by_one_diff() {
        let (dir, pkgs) = tagged_repo_of(5);
        for index in 0..5 {
            commit(dir.path(), &format!("pkg-{index}/src/lib.rs"), "x\n", "feat: x");
        }

        let (result, diffs) = sources_counting_diffs(dir.path(), &pkgs);

        assert_eq!(diffs, 1);
        assert_eq!(files(&result, "pkg-3"), ["pkg-3/src/lib.rs"]);
    }

    /// A budget too small for all the directories splits the listing across diffs without changing what is found.
    #[test]
    fn a_small_pathspec_budget_splits_the_listing_without_changing_it() {
        let (dir, pkgs) = tagged_repo_of(6);
        for index in 0..6 {
            commit(dir.path(), &format!("pkg-{index}/src/lib.rs"), "x\n", "feat: x");
        }
        let graph = FixedGraph { pkgs: pkgs.to_vec() };
        let cfg = crate::config::load(dir.path()).unwrap();
        let run = |budget: usize| {
            let runner = RecordingGit {
                calls: Mutex::new(Vec::new()),
            };
            let git = GitAccess::new(dir.path(), &runner);
            let tags = TagIndex::build(&git, &graph, &Default::default(), &cfg).unwrap();
            let refs: Vec<&Package> = pkgs.iter().collect();
            let result = changed_source_files_in_batches(&refs, &tags, &git, DEFAULT_IGNORED_PATHS, budget).unwrap();
            let diffs = runner
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| call.starts_with("diff "))
                .count();
            (result, diffs)
        };

        let (whole, one) = run(PATHSPEC_BUDGET);
        let (split, several) = run(1);

        assert_eq!(one, 1);
        assert_eq!(several, 6, "a budget of one byte puts each directory in its own diff");
        assert_eq!(whole, split);
    }

    #[test]
    fn two_base_commits_cost_two_diffs() {
        let (dir, pkgs) = tagged_repo_of(4);
        let root = dir.path();
        commit(root, "pkg-0/src/lib.rs", "x\n", "feat: x");
        for name in ["pkg-2", "pkg-3"] {
            run_git(root, &["tag", "-a", "-m", "release", &format!("{name}@1.0.1")]);
        }
        for index in 0..4 {
            commit(root, &format!("pkg-{index}/src/lib.rs"), "y\n", "feat: y");
        }

        let (result, diffs) = sources_counting_diffs(root, &pkgs);

        assert_eq!(diffs, 2);
        assert_eq!(files(&result, "pkg-0"), ["pkg-0/src/lib.rs"]);
        assert_eq!(files(&result, "pkg-3"), ["pkg-3/src/lib.rs"]);
    }

    /// A repo with a root package, a package with a space in its directory, one nested in another and an untagged one.
    fn nested_repo() -> (tempfile::TempDir, Vec<Package>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        let dirs = [".", "pkg-a", "pkg-a/inner", "pkg b", "pkg-c"];
        for directory in dirs {
            let prefix = if directory == "." {
                String::new()
            } else {
                format!("{directory}/")
            };
            write(root, &format!("{prefix}Cargo.toml"), "[package]\n");
            write(root, &format!("{prefix}src/lib.rs"), "\n");
        }
        commit(root, "README.md", "r\n", "chore: init");
        for directory in ["root", "pkg-a", "pkg-a/inner", "pkg-b"] {
            run_git(root, &["tag", "-a", "-m", "release", &format!("{directory}@1.0.0")]);
        }
        commit(root, "pkg-a/src/lib.rs", "a\n", "feat: a");
        commit(root, "pkg-a/inner/src/lib.rs", "i\n", "feat: inner");
        commit(root, "pkg b/src/a b.rs", "b\n", "feat: spaced");
        commit(root, "pkg-a/tests/ignored.rs", "t\n", "test: ignored");
        commit(root, "pkg-c/src/lib.rs", "c\n", "feat: untagged");
        commit(root, "top.rs", "t\n", "feat: top");
        write(root, "pkg-a/inner/src/dirty.rs", "d\n");
        run_git(root, &["add", "pkg-a/inner/src/dirty.rs"]);
        let pkgs = dirs
            .iter()
            .map(|directory| {
                let mut pkg = make_pkg(directory);
                if *directory == "pkg b" {
                    // A tag name cannot hold a space.
                    pkg.id = id("pkg-b");
                }
                pkg
            })
            .collect();
        (dir, pkgs)
    }

    /// The old behaviour: one diff (or `ls-files`, untagged) per package.
    fn per_package_reference(root: &Path, pkgs: &[Package]) -> BTreeMap<PackageId, Vec<PathBuf>> {
        let excludes: Vec<String> = DEFAULT_IGNORED_PATHS.iter().map(|p| exclude_pathspec(p)).collect();
        pkgs.iter()
            .map(|pkg| {
                let tag = format!("{}@1.0.0", pkg.id);
                let tagged = run_git_ok(root, &["rev-parse", "-q", "--verify", &format!("{tag}^{{commit}}")]);
                let mut args: Vec<String> = match &tagged {
                    Some(commit) => [
                        "diff",
                        "--no-renames",
                        "--relative",
                        "--name-only",
                        "-z",
                        commit.trim(),
                        "--",
                    ]
                    .map(String::from)
                    .to_vec(),
                    None => ["ls-files", "-z", "--"].map(String::from).to_vec(),
                };
                args.extend(package_paths(pkg).iter().map(|d| d.to_string_lossy().into_owned()));
                args.extend(excludes.iter().cloned());
                let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                let output = GitRunner.run("git", &refs, root).unwrap();
                let files: BTreeSet<PathBuf> = output
                    .stdout
                    .split('\0')
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from)
                    .collect();
                (pkg.id.clone(), files.into_iter().collect())
            })
            .collect()
    }

    fn run_git_ok(root: &Path, args: &[&str]) -> Option<String> {
        let output = GitRunner.run("git", args, root).unwrap();
        output.success().then_some(output.stdout)
    }

    #[test]
    fn batched_listing_equals_the_per_package_listing() {
        let (dir, pkgs) = nested_repo();

        let (batched, diffs) = sources_counting_diffs(dir.path(), &pkgs);

        assert_eq!(batched, per_package_reference(dir.path(), &pkgs));
        assert_eq!(diffs, 1);
        assert!(batched[&id("root")].iter().any(|p| p == Path::new("top.rs")));
        assert!(!batched[&id("pkg-c")].is_empty(), "untagged lists all its files");
    }

    #[test]
    fn a_nested_package_gets_its_own_files_and_not_its_parents_other_files() {
        let (dir, pkgs) = nested_repo();

        let (result, _) = sources_counting_diffs(dir.path(), &pkgs);

        assert_eq!(
            files(&result, "pkg-a/inner"),
            ["pkg-a/inner/src/dirty.rs", "pkg-a/inner/src/lib.rs"]
        );
        // As before batching, the parent's directory also contains the nested package's files.
        assert!(files(&result, "pkg-a").contains(&"pkg-a/inner/src/lib.rs".to_owned()));
        assert!(files(&result, "pkg-a").contains(&"pkg-a/src/lib.rs".to_owned()));
        assert_eq!(files(&result, "pkg-b"), ["pkg b/src/a b.rs"]);
    }

    #[test]
    fn a_package_without_changes_has_an_empty_listing() {
        let (dir, pkgs) = tagged_repo_of(3);
        commit(dir.path(), "pkg-1/src/lib.rs", "x\n", "feat: x");

        let (result, diffs) = sources_counting_diffs(dir.path(), &pkgs);

        assert_eq!(diffs, 1);
        assert!(result[&id("pkg-0")].is_empty() && result[&id("pkg-2")].is_empty());
        assert_eq!(files(&result, "pkg-1"), ["pkg-1/src/lib.rs"]);
    }

    #[test]
    fn a_source_edit_is_reported() {
        let dir = tagged_repo();
        commit(dir.path(), "pkg-a/src/lib.rs", "a\n", "feat: a");

        let result = sources(dir.path(), &[make_pkg("pkg-a")], DEFAULT_IGNORED_PATHS);

        assert_eq!(files(&result, "pkg-a"), ["pkg-a/src/lib.rs"]);
    }

    #[test]
    fn test_snapshot_and_markdown_edits_are_not_reported() {
        let dir = tagged_repo();
        let root = dir.path();
        write(root, "pkg-a/tests/it.rs", "t\n");
        write(root, "pkg-a/src/snapshots/x.snap", "s\n");
        write(root, "pkg-a/README.md", "r\n");
        write(root, "pkg-a/CHANGELOG.md", "c\n");
        write(root, "pkg-a/benches/b.rs", "b\n");
        write(root, "pkg-a/examples/e.rs", "e\n");
        run_git(root, &["add", "-A"]);
        run_git(root, &["commit", "-q", "-m", "test: only"]);

        let result = sources(root, &[make_pkg("pkg-a")], DEFAULT_IGNORED_PATHS);

        assert!(result[&id("pkg-a")].is_empty(), "{result:?}");
    }

    #[test]
    fn each_ignore_kind_is_excluded_on_its_own_next_to_a_source_edit() {
        let dir = tagged_repo();
        let root = dir.path();
        write(root, "pkg-a/src/a.rs", "a\n");
        write(root, "pkg-a/src/tests/inner.rs", "t\n");
        write(root, "pkg-a/src/y.snap", "s\n");
        write(root, "pkg-a/docs/guide.md", "m\n");
        run_git(root, &["add", "-A"]);
        run_git(root, &["commit", "-q", "-m", "feat: mixed"]);

        let result = sources(root, &[make_pkg("pkg-a")], DEFAULT_IGNORED_PATHS);

        assert_eq!(files(&result, "pkg-a"), ["pkg-a/src/a.rs"]);
    }

    #[test]
    fn a_reverted_edit_is_not_reported() {
        let dir = tagged_repo();
        let root = dir.path();
        commit(root, "pkg-a/src/lib.rs", "a\n", "feat: a");
        run_git(root, &["rm", "-q", "pkg-a/src/lib.rs"]);
        run_git(root, &["commit", "-q", "-m", "revert: a"]);

        let result = sources(root, &[make_pkg("pkg-a")], DEFAULT_IGNORED_PATHS);

        assert!(result[&id("pkg-a")].is_empty());
    }

    #[test]
    fn an_untagged_package_yields_all_its_tracked_source_files() {
        let dir = tagged_repo();
        let root = dir.path();
        commit(root, "pkg-new/Cargo.toml", "[package]\n", "feat: new");
        commit(root, "pkg-new/src/lib.rs", "l\n", "feat: lib");
        commit(root, "pkg-new/tests/t.rs", "t\n", "test: t");

        let result = sources(root, &[make_pkg("pkg-new")], DEFAULT_IGNORED_PATHS);

        assert_eq!(files(&result, "pkg-new"), ["pkg-new/Cargo.toml", "pkg-new/src/lib.rs"]);
    }

    #[test]
    fn only_the_package_that_changed_is_reported() {
        let dir = tagged_repo();
        commit(dir.path(), "pkg-b/src/lib.rs", "b\n", "feat: b");
        let pkgs = [make_pkg("pkg-a"), make_pkg("pkg-b")];

        let result = sources(dir.path(), &pkgs, DEFAULT_IGNORED_PATHS);

        assert!(result[&id("pkg-a")].is_empty());
        assert_eq!(files(&result, "pkg-b"), ["pkg-b/src/lib.rs"]);
    }

    #[test]
    fn an_uncommitted_source_edit_is_reported() {
        let dir = tagged_repo();
        write(dir.path(), "pkg-a/Cargo.toml", "[package]\nname = \"a\"\n");

        let result = sources(dir.path(), &[make_pkg("pkg-a")], DEFAULT_IGNORED_PATHS);

        assert_eq!(files(&result, "pkg-a"), ["pkg-a/Cargo.toml"]);
    }

    #[test]
    fn the_ignore_list_can_be_overridden() {
        let dir = tagged_repo();
        let root = dir.path();
        write(root, "pkg-a/tests/it.rs", "t\n");
        write(root, "pkg-a/README.md", "r\n");
        run_git(root, &["add", "-A"]);
        run_git(root, &["commit", "-q", "-m", "test: it"]);
        let pkgs = [make_pkg("pkg-a")];

        assert_eq!(files(&sources(root, &pkgs, &["*.md"]), "pkg-a"), ["pkg-a/tests/it.rs"]);
        assert_eq!(
            files(&sources(root, &pkgs, &[]), "pkg-a"),
            ["pkg-a/README.md", "pkg-a/tests/it.rs"]
        );
    }

    fn group_table(fixed: &[&[&str]], linked: &[&[&str]]) -> GroupTable {
        use crate::config::groups::GroupMember;
        use callisto_model::{GroupKind, GroupName};
        let defs = |lists: &[&[&str]], kind: GroupKind, prefix: &str| -> Vec<crate::config::GroupDef> {
            lists
                .iter()
                .enumerate()
                .map(|(n, members)| crate::config::GroupDef {
                    name: GroupName(format!("{prefix}{n}")),
                    kind,
                    members: members.iter().map(|m| GroupMember::Package(id(m))).collect(),
                })
                .collect()
        };
        GroupTable::try_from_groups(defs(fixed, GroupKind::Fixed, "f"), defs(linked, GroupKind::Linked, "l")).unwrap()
    }

    fn change_set(names: &[&str]) -> BTreeMap<PackageId, Vec<PathBuf>> {
        names
            .iter()
            .map(|n| (id(n), vec![PathBuf::from("src/lib.rs")]))
            .collect()
    }

    fn covered_set(names: &[&str]) -> BTreeSet<PackageId> {
        names.iter().map(|n| id(n)).collect()
    }

    #[test]
    fn a_changed_package_outside_any_group_without_a_changeset_is_reported() {
        let out = uncovered_changes(&change_set(&["a", "b"]), &covered_set(&["a"]), &GroupTable::default());

        assert_eq!(out, [id("b")]);
    }

    #[test]
    fn a_package_with_no_changed_files_is_not_reported() {
        let mut changed = change_set(&["a"]);
        changed.insert(id("quiet"), Vec::new());

        assert_eq!(
            uncovered_changes(&changed, &covered_set(&[]), &GroupTable::default()),
            [id("a")]
        );
    }

    #[test]
    fn a_covered_fixed_group_member_covers_its_sibling() {
        let groups = group_table(&[&["a", "b"]], &[]);

        assert!(uncovered_changes(&change_set(&["b"]), &covered_set(&["a"]), &groups).is_empty());
        assert_eq!(
            uncovered_changes(&change_set(&["b"]), &covered_set(&[]), &groups),
            [id("b")]
        );
    }

    #[test]
    fn a_covered_linked_group_member_covers_its_sibling() {
        let groups = group_table(&[], &[&["a", "b"]]);

        assert!(uncovered_changes(&change_set(&["b"]), &covered_set(&["a"]), &groups).is_empty());
    }

    #[test]
    fn a_covered_package_in_another_group_does_not_cover() {
        let groups = group_table(&[&["a", "x"]], &[&["b", "y"]]);

        let out = uncovered_changes(&change_set(&["b"]), &covered_set(&["a", "x"]), &groups);

        assert_eq!(out, [id("b")]);
    }

    mod props {
        use proptest::prelude::*;

        use super::*;

        const NAMES: [&str; 8] = ["p0", "p1", "p2", "p3", "p4", "p5", "p6", "p7"];

        proptest! {
            #[test]
            fn uncovered_is_a_changed_subset_with_no_covered_package_or_group_mate(
                changed_mask in prop::collection::vec(any::<bool>(), 8),
                covered_mask in prop::collection::vec(any::<bool>(), 8),
                fixed_cut in 0usize..=8,
                linked_cut in 0usize..=8,
            ) {
                let pick = |mask: &[bool]| -> Vec<&str> {
                    NAMES.iter().zip(mask).filter(|(_, on)| **on).map(|(n, _)| *n).collect()
                };
                let changed_names = pick(&changed_mask);
                let covered_names = pick(&covered_mask);
                let (fixed_lo, fixed_hi) = (0, fixed_cut.min(4));
                let (linked_lo, linked_hi) = (4, linked_cut.max(4));
                let fixed: Vec<&str> = NAMES[fixed_lo..fixed_hi].to_vec();
                let linked: Vec<&str> = NAMES[linked_lo..linked_hi].to_vec();
                let fixed_groups: Vec<&[&str]> = if fixed.is_empty() { vec![] } else { vec![&fixed] };
                let linked_groups: Vec<&[&str]> = if linked.is_empty() { vec![] } else { vec![&linked] };
                let groups = group_table(&fixed_groups, &linked_groups);
                let changed = change_set(&changed_names);
                let covered = covered_set(&covered_names);

                let out = uncovered_changes(&changed, &covered, &groups);

                for pkg in &out {
                    prop_assert!(changed.contains_key(pkg));
                    prop_assert!(!covered.contains(pkg));
                    for members in [&fixed, &linked] {
                        if members.iter().any(|m| id(m) == *pkg) {
                            prop_assert!(!members.iter().any(|m| covered.contains(&id(m))));
                        }
                    }
                }
            }
        }
    }
}
