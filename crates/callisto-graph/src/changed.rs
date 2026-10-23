use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use callisto_model::vcs::{GitAccess, VcsError};
use callisto_model::{Package, PackageId};

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
            let directory = if directory.as_path() == Path::new(".") {
                String::new()
            } else {
                directory.to_string_lossy().into_owned()
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
}
