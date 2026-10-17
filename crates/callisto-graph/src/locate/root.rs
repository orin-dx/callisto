use std::path::{Path, PathBuf};

use callisto_model::vcs::{GitAccess, VcsError};
use callisto_model::CommandRunner;

use crate::locate::membership::{parse_error, read_root_manifest};
use crate::locate::LocateError;

/// Finds the root between `start` and its Git toplevel (never above it): the nearest
/// workspace marker, else the outermost package manifest so vendored packages can't shadow it.
///
/// The toplevel comes from `git rev-parse --show-toplevel` (via `runner`), not a hand-rolled
/// walk for a `.git` entry -- `git` alone honors `GIT_DIR`/`GIT_WORK_TREE`/
/// `GIT_CEILING_DIRECTORIES` the way every other Git-aware tool does.
pub fn find_workspace_root(start: &Path, runner: &dyn CommandRunner) -> Result<PathBuf, LocateError> {
    let canonical = dunce::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let toplevel = GitAccess::new(&canonical, runner).toplevel().map_err(|e| match &e {
        VcsError::Git(message) if message.to_lowercase().contains("not a git repository") => {
            LocateError::NotAGitRepository {
                start: start.to_path_buf(),
            }
        }
        _ => LocateError::from(e),
    })?;
    let range = || canonical.ancestors().take_while(|dir| dir.starts_with(&toplevel));
    for dir in range() {
        if is_workspace_root(dir)? {
            return Ok(dir.to_path_buf());
        }
    }
    let mut outermost = None;
    for dir in range() {
        if is_package_root(dir)? {
            outermost = Some(dir);
        }
    }
    outermost
        .map(Path::to_path_buf)
        .ok_or_else(|| LocateError::WorkspaceRootNotFound {
            start: start.to_path_buf(),
            toplevel: toplevel.clone(),
        })
}

/// Loads `dir/Cargo.toml` as a TOML document, or `None` if it does not exist. A manifest that cannot be read or
/// parsed is an error: skipping it would let the search settle on a different root.
fn cargo_document(dir: &Path) -> Result<Option<toml_edit::DocumentMut>, LocateError> {
    let path = dir.join("Cargo.toml");
    read_root_manifest(&path)?
        .map(|content| {
            content
                .parse::<toml_edit::DocumentMut>()
                .map_err(|error| parse_error(&path, error.to_string()))
        })
        .transpose()
}

fn path_exists(path: &Path) -> Result<bool, LocateError> {
    callisto_model::strict_fs::exists(path).map_err(|error| LocateError::ManifestReadError {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

/// True when `key` names a table declared with its own header (`[key]`), not a table that
/// exists only implicitly as a dotted-key parent (e.g. `[key.sub]` alone does not count) or
/// text that merely mentions the name inside a comment.
fn has_explicit_table(doc: &toml_edit::DocumentMut, key: &str) -> bool {
    doc.get(key)
        .and_then(|item| item.as_table())
        .is_some_and(|t| !t.is_implicit())
}

/// A single-package repository root: `Cargo.toml` with `[package]`, `package.json`, or `pyproject.toml`.
fn is_package_root(dir: &Path) -> Result<bool, LocateError> {
    if cargo_document(dir)?.is_some_and(|doc| has_explicit_table(&doc, "package")) {
        return Ok(true);
    }
    Ok(path_exists(&dir.join("package.json"))? || path_exists(&dir.join("pyproject.toml"))?)
}

fn is_workspace_root(dir: &Path) -> Result<bool, LocateError> {
    if cargo_document(dir)?.is_some_and(|doc| has_explicit_table(&doc, "workspace")) {
        return Ok(true);
    }

    let pkg_json = dir.join("package.json");
    if let Some(content) = read_root_manifest(&pkg_json)? {
        let value = serde_json::from_str::<serde_json::Value>(&content)
            .map_err(|error| parse_error(&pkg_json, error.to_string()))?;
        if value.get("workspaces").is_some() {
            return Ok(true);
        }
    }

    Ok(path_exists(&dir.join("pnpm-workspace.yaml"))? || dir.join(".moon").is_dir())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use callisto_model::{CommandError, CommandOutput, CommandRunner};

    use super::*;

    fn write(root: &Path, path: &str, content: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// A tempdir holding a `.git` marker: the Git toplevel.
    fn repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        (dir, root)
    }

    /// Stands in for the real `git` binary's `rev-parse --show-toplevel`, so these tests stay
    /// deterministic and don't require a real repository on disk. It answers by walking `cwd`'s
    /// ancestors for the nearest `.git` entry (file or directory), the same observable result
    /// real `git` gives for these fixtures -- `find_workspace_root` itself is what's under test
    /// here, not `git`'s own toplevel resolution.
    struct FakeGitToplevel;

    impl CommandRunner for FakeGitToplevel {
        fn run(&self, _program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
            assert_eq!(args, ["rev-parse", "--show-toplevel"]);
            match cwd.ancestors().find(|dir| dir.join(".git").exists()) {
                Some(toplevel) => Ok(CommandOutput {
                    exit_code: Some(0),
                    stdout: format!("{}\n", toplevel.display()),
                    stderr: String::new(),
                }),
                None => Ok(CommandOutput {
                    exit_code: Some(128),
                    stdout: String::new(),
                    stderr: "fatal: not a git repository (or any of the parent directories): .git".to_string(),
                }),
            }
        }
    }

    fn find(start: &Path) -> Result<PathBuf, LocateError> {
        find_workspace_root(start, &FakeGitToplevel)
    }

    const CRATE: &str = "[package]\nname = \"app\"\nversion = \"1.0.0\"\n";

    #[test]
    fn a_single_package_at_the_toplevel_is_the_root_from_a_subdirectory() {
        for (manifest, content) in [
            ("Cargo.toml", CRATE),
            ("package.json", r#"{"name":"app","version":"1.0.0"}"#),
            ("pyproject.toml", "[project]\nname = \"app\"\nversion = \"1.0.0\"\n"),
        ] {
            let (_dir, root) = repo();
            write(&root, manifest, content);
            write(&root, "src/deep/file.txt", "");
            assert_eq!(find(&root.join("src/deep")).unwrap(), root, "{manifest}");
        }
    }

    // A vendored package's own manifest must not shadow the repository's.
    #[test]
    fn the_outermost_package_wins_over_a_vendored_one() {
        let (_dir, root) = repo();
        write(&root, "Cargo.toml", CRATE);
        write(&root, "vendor/some-dep/Cargo.toml", "[package]\nname = \"some-dep\"\n");
        write(&root, "node_modules/lib/package.json", r#"{"name":"lib"}"#);
        assert_eq!(find(&root.join("vendor/some-dep")).unwrap(), root);
        assert_eq!(find(&root.join("node_modules/lib")).unwrap(), root);
    }

    // Discovery never climbs past the Git toplevel to an unrelated parent manifest.
    #[test]
    fn discovery_stops_at_the_git_toplevel() {
        let outer = tempfile::tempdir().unwrap();
        write(outer.path(), "package.json", r#"{"name":"unrelated"}"#);
        write(outer.path(), "proj/.git/HEAD", "");
        write(outer.path(), "proj/docs/readme.md", "");
        let start = outer.path().join("proj/docs");
        let error = find(&start).unwrap_err();
        let toplevel = dunce::canonicalize(outer.path().join("proj")).unwrap();
        assert_eq!(
            error,
            LocateError::WorkspaceRootNotFound {
                start: start.clone(),
                toplevel: toplevel.clone()
            }
        );
        assert!(error.to_string().contains(&toplevel.display().to_string()));
    }

    // A manifest that cannot be understood must stop the search, not let it settle on a different root.
    #[test]
    fn a_malformed_manifest_on_the_way_up_is_an_error_not_a_reason_to_pick_another_root() {
        let (_dir, root) = repo();
        write(&root, "Cargo.toml", "[workspace]\nmembers = [\"member\"]\n");
        write(&root, "member/Cargo.toml", "[package\nname = \"member\"\n");
        let error = find(&root.join("member")).unwrap_err();
        assert!(
            matches!(&error, LocateError::ManifestParseError { path, .. } if path.ends_with("member/Cargo.toml")),
            "{error:?}"
        );
    }

    #[test]
    fn a_root_package_json_that_is_not_json_or_not_utf8_is_an_error() {
        let (_dir, root) = repo();
        write(&root, "package.json", "{\"workspaces\": [\"packages/*\"],}");
        let error = find(&root).unwrap_err();
        assert!(matches!(error, LocateError::ManifestParseError { .. }), "{error:?}");

        fs::write(root.join("package.json"), b"{\"name\":\"caf\xe9\"}").unwrap();
        let error = find(&root).unwrap_err();
        assert!(matches!(error, LocateError::ManifestReadError { .. }), "{error:?}");
    }

    #[test]
    fn a_workspace_root_manifest_with_a_byte_order_mark_is_still_a_workspace_root() {
        let (_dir, root) = repo();
        write(&root, "Cargo.toml", "\u{FEFF}[workspace]\nmembers = [\"member\"]\n");
        write(&root, "member/Cargo.toml", "[package]\nname = \"member\"\n");
        assert_eq!(find(&root.join("member")).unwrap(), root);
    }

    #[test]
    fn a_workspace_marker_beats_a_nearer_member_manifest() {
        let (_dir, root) = repo();
        write(&root, "Cargo.toml", "[workspace]\nmembers = [\"member\"]\n");
        write(&root, "member/Cargo.toml", "[package]\nname = \"member\"\n");
        assert_eq!(find(&root.join("member")).unwrap(), root);
    }

    // A submodule's `.git` file makes it its own toplevel.
    #[test]
    fn a_nested_repository_stops_at_its_own_toplevel() {
        let (_dir, root) = repo();
        write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
        write(&root, "sub/.git", "gitdir: ../.git/modules/sub\n");
        write(&root, "sub/package.json", r#"{"name":"sub"}"#);
        write(&root, "sub/src/index.js", "");
        assert_eq!(find(&root.join("sub/src")).unwrap(), root.join("sub"));
    }

    #[test]
    fn outside_git_names_the_fix() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Cargo.toml", CRATE);
        let error = find(dir.path()).unwrap_err();
        assert!(matches!(error, LocateError::NotAGitRepository { .. }), "{error}");
        let help = miette::Diagnostic::help(&error).unwrap().to_string();
        assert!(help.contains("git init"), "{help}");
    }

    /// Real-`git` integration: `find_workspace_root` must actually shell `git rev-parse
    /// --show-toplevel` and use its answer, not merely accept anything a `CommandRunner`
    /// returns -- proven end to end against a genuine `git init`-created repository.
    #[test]
    fn integrates_with_a_real_git_repository() {
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

        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success(),
            "git init must succeed for this test to be meaningful"
        );
        write(&root, "Cargo.toml", CRATE);
        write(&root, "src/deep/file.txt", "");

        assert_eq!(
            find_workspace_root(&root.join("src/deep"), &RealGitRunner).unwrap(),
            root
        );
    }

    // A comment merely mentioning "[workspace]" must not be text-matched as a real table.
    #[test]
    fn a_commented_out_workspace_mention_does_not_count_as_a_workspace_root() {
        let (_dir, root) = repo();
        write(&root, "Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
        write(
            &root,
            "crates/a/Cargo.toml",
            "[package]\nname = \"a\"\nversion = \"1.0.0\"\n# not a workspace root; see [workspace] in ../../Cargo.toml\n",
        );
        assert_eq!(find(&root.join("crates/a")).unwrap(), root);
    }

    // `[workspace.package]` alone declares no `[workspace]` table of its own.
    #[test]
    fn workspace_package_table_alone_does_not_count_as_a_workspace_root() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Cargo.toml", "[workspace.package]\nedition = \"2021\"\n");
        assert!(!is_workspace_root(dir.path()).unwrap());
    }

    #[test]
    fn a_cargo_toml_without_package_or_workspace_is_not_a_root() {
        let (_dir, root) = repo();
        write(&root, "Cargo.toml", "[dependencies]\n");
        assert!(matches!(find(&root), Err(LocateError::WorkspaceRootNotFound { .. })));
    }
}
