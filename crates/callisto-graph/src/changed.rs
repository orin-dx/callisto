use std::path::{Path, PathBuf};

use callisto_model::{CommandRunner, Package};
use callisto_vcs::{GitAccess, GitDataSource};

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

pub fn changed_since_last_tag<R: CommandRunner>(
    runner: &R,
    root: &Path,
    pkg: &Package,
    tags: &TagIndex,
    git: &GitAccess<'_>,
) -> Result<bool, GraphError> {
    let Some(last) = tags.last_tag(&pkg.id) else {
        return Ok(true);
    };

    // `git` is a single `GitAccess` built once by the caller and shared
    // across every package -- rediscovering it per package (native gix
    // repository-open, or a fresh `ShellGit` on the fallback path) for an
    // N-package workspace was N redundant discoveries of the exact same
    // repository. A failure on either backend is not fatal -- it just means
    // the cheap short-circuit below is skipped in favor of the exact `git
    // diff --quiet` check.
    //
    // `last.name` is a tag read back from the repository's own tag list
    // (via `TagIndex`), not text Callisto renders itself -- unlike a
    // `tag-template`-rendered name, it is not run through
    // `is_valid_git_ref_name`. It is fully qualified as `refs/tags/<name>`
    // before being shelled to `git log`/`git diff` so it can never be
    // misread as a CLI flag by either command's argument parser, even in
    // the (currently unreachable, given `TagTemplate::parse`'s and
    // `PackageId::parse`'s own leading-hyphen rejections) case of a
    // hyphen-leading tag reaching this far.
    let qualified = format!("refs/tags/{}", last.name.as_str());

    if let Ok(commits) = git.commits_since(Some(&qualified), &[]) {
        if !commits.is_empty() {
            return Ok(true);
        }
    }

    let paths = package_paths(pkg);
    let mut args = vec!["diff", "--quiet", qualified.as_str(), "--"];
    let path_strs: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    for p in &path_strs {
        args.push(p);
    }

    let output = runner.run("git", &args, root)?;
    Ok(!output.success())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use callisto_model::{
        CommandError, CommandOutput, DepEdge, ManifestDecl, ManifestFormat, ManifestRole, PackageId,
    };

    use super::*;
    use crate::resolver::DependencyResolver;

    fn make_pkg(name: &str) -> Package {
        let manifest = ManifestDecl::new(
            format!("{name}/Cargo.toml"),
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        Package {
            id: PackageId::parse(name).unwrap(),
            manifests: vec![manifest],
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

    /// A directory that is guaranteed not to sit inside any Git repository,
    /// so `callisto_vcs::GitRepository::discover` fails exactly the way it
    /// unconditionally does on `wasm32` -- forcing every path under test
    /// through the `CommandRunner` fallback, the same fixture pattern
    /// `tags.rs`'s tests use.
    fn non_repo_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            callisto_vcs::GitRepository::discover(dir.path()).is_err(),
            "test fixture must not be discoverable as a Git repo"
        );
        dir
    }

    fn tag_index_with_tag(dir: &std::path::Path, pkg_name: &str, tag: &str) -> TagIndex {
        struct TagListRunner(String);
        impl CommandRunner for TagListRunner {
            fn run(
                &self,
                program: &str,
                args: &[&str],
                _cwd: &Path,
            ) -> Result<CommandOutput, CommandError> {
                assert_eq!(program, "git");
                assert_eq!(args, ["tag", "--list"]);
                Ok(CommandOutput {
                    exit_code: Some(0),
                    stdout: self.0.clone(),
                    stderr: String::new(),
                })
            }
        }
        let graph = FixedGraph {
            pkgs: vec![make_pkg(pkg_name)],
        };
        let cfg = crate::config::load(dir).unwrap();
        let runner = TagListRunner(tag.to_string());
        let git = GitAccess::discover(dir, &runner);
        TagIndex::build(&git, &graph, &cfg).unwrap()
    }

    /// Routes `git log` (the `commits_since` short-circuit) and `git diff
    /// --quiet` (the exact fallback check) to independently canned
    /// responses, counting each kind of invocation separately and
    /// recording the exact args of the most recent call of each kind so
    /// tests can inspect exactly what was shelled.
    struct RoutingRunner {
        log_calls: AtomicUsize,
        diff_calls: AtomicUsize,
        log_stdout: String,
        diff_exit_code: i32,
        last_log_args: std::sync::Mutex<Vec<String>>,
        last_diff_args: std::sync::Mutex<Vec<String>>,
    }

    impl CommandRunner for RoutingRunner {
        fn run(
            &self,
            program: &str,
            args: &[&str],
            _cwd: &Path,
        ) -> Result<CommandOutput, CommandError> {
            assert_eq!(program, "git");
            match args.first() {
                Some(&"log") => {
                    self.log_calls.fetch_add(1, Ordering::SeqCst);
                    *self.last_log_args.lock().unwrap() =
                        args.iter().map(|s| s.to_string()).collect();
                    Ok(CommandOutput {
                        exit_code: Some(0),
                        stdout: self.log_stdout.clone(),
                        stderr: String::new(),
                    })
                }
                Some(&"diff") => {
                    self.diff_calls.fetch_add(1, Ordering::SeqCst);
                    *self.last_diff_args.lock().unwrap() =
                        args.iter().map(|s| s.to_string()).collect();
                    Ok(CommandOutput {
                        exit_code: Some(self.diff_exit_code),
                        stdout: String::new(),
                        stderr: String::new(),
                    })
                }
                other => panic!("unexpected git subcommand: {other:?}"),
            }
        }
    }

    fn routing_runner(log_stdout: String, diff_exit_code: i32) -> RoutingRunner {
        RoutingRunner {
            log_calls: AtomicUsize::new(0),
            diff_calls: AtomicUsize::new(0),
            log_stdout,
            diff_exit_code,
            last_log_args: std::sync::Mutex::new(Vec::new()),
            last_diff_args: std::sync::Mutex::new(Vec::new()),
        }
    }

    #[test]
    fn returns_true_immediately_when_package_has_no_last_tag() {
        let dir = non_repo_dir();
        let pkg = make_pkg("pkg-a");
        let tags = TagIndex::empty();
        let runner = routing_runner(String::new(), 0);
        let git = GitAccess::discover(dir.path(), &runner);

        let changed = changed_since_last_tag(&runner, dir.path(), &pkg, &tags, &git).unwrap();

        assert!(changed, "a package with no last tag must count as changed");
        assert_eq!(
            runner.log_calls.load(Ordering::SeqCst),
            0,
            "must short-circuit before shelling any git command"
        );
        assert_eq!(runner.diff_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn short_circuits_true_when_commits_since_finds_commits_no_diff_check_needed() {
        let dir = non_repo_dir();
        let pkg = make_pkg("pkg-a");
        let tags = tag_index_with_tag(dir.path(), "pkg-a", "pkg-a@1.0.0");
        // One well-formed `git log --format=<RS>%H<FS>%B` record.
        let sha = "a".repeat(40);
        let log_stdout = format!("\u{1e}{sha}\u{1f}feat: something\n");
        let runner = routing_runner(log_stdout, 0);
        let git = GitAccess::discover(dir.path(), &runner);

        let changed = changed_since_last_tag(&runner, dir.path(), &pkg, &tags, &git).unwrap();

        assert!(changed);
        assert_eq!(runner.log_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            runner.diff_calls.load(Ordering::SeqCst),
            0,
            "commits_since already found commits; diff --quiet must not run"
        );
        assert!(
            runner
                .last_log_args
                .lock()
                .unwrap()
                .iter()
                .any(|a| a == "refs/tags/pkg-a@1.0.0..HEAD"),
            "the tag must be shelled as a fully-qualified refs/tags/ ref, not a bare name that \
             a maliciously-named tag could get misread as a `git log` flag, got: {:?}",
            runner.last_log_args.lock().unwrap()
        );
    }

    #[test]
    fn falls_back_to_diff_quiet_when_commits_since_is_empty() {
        let dir = non_repo_dir();
        let pkg = make_pkg("pkg-a");
        let tags = tag_index_with_tag(dir.path(), "pkg-a", "pkg-a@1.0.0");
        let runner = routing_runner(String::new(), 1); // non-zero exit == files differ == changed
        let git = GitAccess::discover(dir.path(), &runner);

        let changed = changed_since_last_tag(&runner, dir.path(), &pkg, &tags, &git).unwrap();

        assert!(changed, "non-zero diff --quiet exit must mean changed");
        assert_eq!(runner.log_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runner.diff_calls.load(Ordering::SeqCst), 1);
        assert!(
            runner
                .last_diff_args
                .lock()
                .unwrap()
                .iter()
                .any(|a| a == "refs/tags/pkg-a@1.0.0"),
            "the tag must be shelled as a fully-qualified refs/tags/ ref in the `git diff` \
             positional too, got: {:?}",
            runner.last_diff_args.lock().unwrap()
        );
    }

    #[test]
    fn diff_quiet_success_exit_means_unchanged() {
        let dir = non_repo_dir();
        let pkg = make_pkg("pkg-a");
        let tags = tag_index_with_tag(dir.path(), "pkg-a", "pkg-a@1.0.0");
        let runner = routing_runner(String::new(), 0); // zero exit == no differences == unchanged
        let git = GitAccess::discover(dir.path(), &runner);

        let changed = changed_since_last_tag(&runner, dir.path(), &pkg, &tags, &git).unwrap();

        assert!(!changed);
    }

    /// The refactor's actual contract: a single `GitAccess`, built once,
    /// must be reusable across multiple packages/calls without any
    /// per-call setup cost of its own -- `changed_since_last_tag` no longer
    /// discovers its own `GitAccess` internally (that responsibility moved
    /// to the caller, `status()`), so calling it repeatedly against one
    /// shared instance must cost exactly one `git log` + one `git diff`
    /// round trip per package, not more.
    #[test]
    fn shared_git_access_is_reusable_across_multiple_packages() {
        let dir = non_repo_dir();
        let pkg_a = make_pkg("pkg-a");
        let pkg_b = make_pkg("pkg-b");
        let tags_a = tag_index_with_tag(dir.path(), "pkg-a", "pkg-a@1.0.0");
        let tags_b = tag_index_with_tag(dir.path(), "pkg-b", "pkg-b@1.0.0");
        let runner = routing_runner(String::new(), 0);
        // Built once, exactly as `status()` now does before its per-package loop.
        let git = GitAccess::discover(dir.path(), &runner);

        changed_since_last_tag(&runner, dir.path(), &pkg_a, &tags_a, &git).unwrap();
        changed_since_last_tag(&runner, dir.path(), &pkg_b, &tags_b, &git).unwrap();

        assert_eq!(
            runner.log_calls.load(Ordering::SeqCst),
            2,
            "exactly one git log per package"
        );
        assert_eq!(
            runner.diff_calls.load(Ordering::SeqCst),
            2,
            "exactly one git diff per package"
        );
    }
}
