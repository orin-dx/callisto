use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use callisto_model::{Package, PackageId};
use callisto_vcs::{GitAccess, VcsError};

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

/// Whether each of `packages` changed since its last tag: a non-merge commit
/// in `tag..HEAD` touched one of its paths, or its tracked files differ from
/// the tag. An untagged package counts as changed.
///
/// One `git rev-parse` for every tag, then per distinct tag commit one `git
/// log` and at most one `git diff`, however many packages share that commit.
pub fn changed_since_last_tag(
    packages: &[&Package],
    tags: &TagIndex,
    git: &GitAccess<'_>,
) -> Result<BTreeMap<PackageId, bool>, GraphError> {
    let mut changed = BTreeMap::new();
    let mut tagged = Vec::new();
    for pkg in packages {
        match tags.last_tag(&pkg.id) {
            // Fully qualified so a tag name can never be misread as a flag.
            Some(last) => tagged.push((*pkg, format!("refs/tags/{}", last.name.as_str()))),
            None => {
                changed.insert(pkg.id.clone(), true);
            }
        }
    }
    if tagged.is_empty() {
        return Ok(changed);
    }

    let refs: Vec<String> = tagged.iter().map(|(_, r)| r.clone()).collect();
    let keys: Vec<String> = match git.resolve_commits(&refs)? {
        Some(shas) => shas.iter().map(|sha| sha.as_str().to_string()).collect(),
        None => refs,
    };
    let mut groups: BTreeMap<String, Vec<&Package>> = BTreeMap::new();
    for ((pkg, _), key) in tagged.into_iter().zip(keys) {
        groups.entry(key).or_default().push(pkg);
    }

    for (since, members) in groups {
        // A failed walk only skips the cheap check in favor of the exact diff.
        let committed = match git.paths_changed_since(&since) {
            Err(VcsError::Command(e)) => return Err(VcsError::Command(e).into()),
            result => result.unwrap_or_default(),
        };
        let mut differing: Option<Option<Vec<PathBuf>>> = None;
        for pkg in members {
            let specs = package_paths(pkg);
            let mut is_changed = touches(&committed, &specs);
            if !is_changed {
                if differing.is_none() {
                    differing = Some(match git.paths_differing_from(&since) {
                        Ok(paths) => Some(paths),
                        Err(VcsError::Command(e)) => return Err(VcsError::Command(e).into()),
                        Err(_) => None,
                    });
                }
                // A diff that could not run counts as changed.
                is_changed = differing
                    .as_ref()
                    .and_then(Option::as_ref)
                    .is_none_or(|paths| touches(paths, &specs));
            }
            changed.insert(pkg.id.clone(), is_changed);
        }
    }
    Ok(changed)
}

/// Whether any of `paths` falls under one of `specs` as a literal `git`
/// pathspec would match it; `.` matches every path.
fn touches(paths: &[PathBuf], specs: &[PathBuf]) -> bool {
    paths.iter().any(|path| {
        specs
            .iter()
            .any(|spec| spec.as_path() == Path::new(".") || path.starts_with(spec))
    })
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
        let tags = TagIndex::build(&git, &graph, &cfg).unwrap();
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

    /// Only the package a commit touched is changed, and packages sharing a
    /// tag commit share one `git log` and one `git diff`.
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
        assert_eq!(count("rev-parse"), 1, "{calls:?}");
        assert_eq!(count("log"), 1, "{calls:?}");
        assert_eq!(count("diff"), 1, "{calls:?}");
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

    /// A commit that touched the package still counts after a later commit
    /// reverted it, like `git log -- <paths>`.
    #[test]
    fn a_reverted_commit_still_counts() {
        let dir = tagged_repo();
        let root = dir.path();
        commit(root, "pkg-a/x.txt", "x\n", "feat: x");
        run_git(root, &["rm", "-q", "pkg-a/x.txt"]);
        run_git(root, &["commit", "-q", "-m", "revert: x"]);
        let pkgs = [make_pkg("pkg-a")];

        let (result, _) = changed(root, &pkgs);

        assert!(result[&id("pkg-a")]);
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

    /// Packages tagged at different commits each get their own range.
    #[test]
    fn packages_tagged_at_different_commits_use_their_own_range() {
        let dir = tagged_repo();
        let root = dir.path();
        commit(root, "pkg-a/f.txt", "1\n", "feat: a before b's release");
        run_git(root, &["tag", "-a", "-m", "release", "pkg-b@1.1.0"]);
        let pkgs = [make_pkg("pkg-a"), make_pkg("pkg-b")];

        let (result, calls) = changed(root, &pkgs);

        assert!(result[&id("pkg-a")]);
        assert!(!result[&id("pkg-b")]);
        assert_eq!(calls.iter().filter(|c| c.starts_with("log")).count(), 2, "{calls:?}");
    }

    /// The root-level package's `.` pathspec matches every path.
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

    #[test]
    fn touches_matches_components_not_string_prefixes() {
        let specs = [PathBuf::from("crates/a")];
        assert!(touches(&[PathBuf::from("crates/a")], &specs));
        assert!(touches(&[PathBuf::from("crates/a/src/lib.rs")], &specs));
        assert!(!touches(&[PathBuf::from("crates/ab/lib.rs")], &specs));
        assert!(touches(&[PathBuf::from("anything")], &[PathBuf::from(".")]));
        assert!(!touches(&[], &[PathBuf::from(".")]));
    }
}
