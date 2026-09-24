use std::fs;
use std::path::{Path, PathBuf};

use crate::locate::LocateError;

/// The nearest ancestor with a workspace marker, else the nearest one holding a package manifest.
pub fn find_workspace_root(start: &Path) -> Result<PathBuf, LocateError> {
    let canonical = dunce::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let nearest = |accept: fn(&Path) -> bool| canonical.ancestors().find(|dir| accept(dir)).map(Path::to_path_buf);
    nearest(is_workspace_root)
        .or_else(|| nearest(is_package_root))
        .ok_or_else(|| LocateError::WorkspaceRootNotFound {
            start: start.to_path_buf(),
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

    #[test]
    fn a_single_package_directory_is_a_root() {
        for (manifest, content) in [
            ("Cargo.toml", "[package]\nname = \"app\"\nversion = \"1.0.0\"\n"),
            ("package.json", r#"{"name":"app","version":"1.0.0"}"#),
            ("pyproject.toml", "[project]\nname = \"app\"\nversion = \"1.0.0\"\n"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            write(dir.path(), manifest, content);
            write(dir.path(), "src/deep/file.txt", "");
            let root = dunce::canonicalize(dir.path()).unwrap();
            assert_eq!(find_workspace_root(&root.join("src/deep")).unwrap(), root, "{manifest}");
        }
    }

    #[test]
    fn a_workspace_marker_beats_a_nearer_member_manifest() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Cargo.toml", "[workspace]\nmembers = [\"member\"]\n");
        write(dir.path(), "member/Cargo.toml", "[package]\nname = \"member\"\n");
        let root = dunce::canonicalize(dir.path()).unwrap();
        assert_eq!(find_workspace_root(&root.join("member")).unwrap(), root);
    }

    #[test]
    fn a_cargo_toml_without_package_or_workspace_is_not_a_root() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Cargo.toml", "[dependencies]\n");
        assert!(matches!(
            find_workspace_root(dir.path()),
            Err(LocateError::WorkspaceRootNotFound { .. })
        ));
    }
}
