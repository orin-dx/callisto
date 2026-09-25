use std::fs;
use std::path::{Path, PathBuf};

use crate::locate::LocateError;

/// Finds the root between `start` and its Git toplevel (never above it): the nearest
/// workspace marker, else the outermost package manifest so vendored packages can't shadow it.
pub fn find_workspace_root(start: &Path) -> Result<PathBuf, LocateError> {
    let canonical = dunce::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let toplevel = canonical
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .ok_or_else(|| LocateError::NotAGitRepository {
            start: start.to_path_buf(),
        })?;
    let range = || canonical.ancestors().take_while(|dir| dir.starts_with(toplevel));
    range()
        .find(|dir| is_workspace_root(dir))
        .or_else(|| range().filter(|dir| is_package_root(dir)).last())
        .map(Path::to_path_buf)
        .ok_or_else(|| LocateError::WorkspaceRootNotFound {
            start: start.to_path_buf(),
            toplevel: toplevel.to_path_buf(),
        })
}

/// A single-package repository root: `Cargo.toml` with `[package]`, `package.json`, or `pyproject.toml`.
fn is_package_root(dir: &Path) -> bool {
    let cargo = fs::read_to_string(dir.join("Cargo.toml")).is_ok_and(|content| content.contains("[package]"));
    cargo || dir.join("package.json").is_file() || dir.join("pyproject.toml").is_file()
}

fn is_workspace_root(dir: &Path) -> bool {
    let cargo_toml = dir.join("Cargo.toml");
    if cargo_toml.exists() {
        if let Ok(content) = fs::read_to_string(&cargo_toml) {
            if content.contains("[workspace]") {
                return true;
            }
        }
    }

    let pkg_json = dir.join("package.json");
    if pkg_json.exists() {
        if let Ok(content) = fs::read_to_string(&pkg_json) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if val.get("workspaces").is_some() {
                    return true;
                }
            }
        }
    }

    if dir.join("pnpm-workspace.yaml").exists() {
        return true;
    }

    if dir.join(".moon").is_dir() {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
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
            assert_eq!(find_workspace_root(&root.join("src/deep")).unwrap(), root, "{manifest}");
        }
    }

    // A vendored package's own manifest must not shadow the repository's.
    #[test]
    fn the_outermost_package_wins_over_a_vendored_one() {
        let (_dir, root) = repo();
        write(&root, "Cargo.toml", CRATE);
        write(&root, "vendor/some-dep/Cargo.toml", "[package]\nname = \"some-dep\"\n");
        write(&root, "node_modules/lib/package.json", r#"{"name":"lib"}"#);
        assert_eq!(find_workspace_root(&root.join("vendor/some-dep")).unwrap(), root);
        assert_eq!(find_workspace_root(&root.join("node_modules/lib")).unwrap(), root);
    }

    // Discovery never climbs past the Git toplevel to an unrelated parent manifest.
    #[test]
    fn discovery_stops_at_the_git_toplevel() {
        let outer = tempfile::tempdir().unwrap();
        write(outer.path(), "package.json", r#"{"name":"unrelated"}"#);
        write(outer.path(), "proj/.git/HEAD", "");
        write(outer.path(), "proj/docs/readme.md", "");
        let start = outer.path().join("proj/docs");
        let error = find_workspace_root(&start).unwrap_err();
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

    #[test]
    fn a_workspace_marker_beats_a_nearer_member_manifest() {
        let (_dir, root) = repo();
        write(&root, "Cargo.toml", "[workspace]\nmembers = [\"member\"]\n");
        write(&root, "member/Cargo.toml", "[package]\nname = \"member\"\n");
        assert_eq!(find_workspace_root(&root.join("member")).unwrap(), root);
    }

    // A submodule's `.git` file makes it its own toplevel.
    #[test]
    fn a_nested_repository_stops_at_its_own_toplevel() {
        let (_dir, root) = repo();
        write(&root, "Cargo.toml", "[workspace]\nmembers = []\n");
        write(&root, "sub/.git", "gitdir: ../.git/modules/sub\n");
        write(&root, "sub/package.json", r#"{"name":"sub"}"#);
        write(&root, "sub/src/index.js", "");
        assert_eq!(find_workspace_root(&root.join("sub/src")).unwrap(), root.join("sub"));
    }

    #[test]
    fn outside_git_names_the_fix() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Cargo.toml", CRATE);
        let error = find_workspace_root(dir.path()).unwrap_err();
        assert!(matches!(error, LocateError::NotAGitRepository { .. }), "{error}");
        let help = miette::Diagnostic::help(&error).unwrap().to_string();
        assert!(help.contains("git init"), "{help}");
    }

    #[test]
    fn a_cargo_toml_without_package_or_workspace_is_not_a_root() {
        let (_dir, root) = repo();
        write(&root, "Cargo.toml", "[dependencies]\n");
        assert!(matches!(
            find_workspace_root(&root),
            Err(LocateError::WorkspaceRootNotFound { .. })
        ));
    }
}
