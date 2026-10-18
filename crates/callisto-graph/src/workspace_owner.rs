//! The workspace that owns a package, resolved once and used everywhere a command must run "in the workspace".
//!
//! Each tool reads its project configuration (`.npmrc`, registries, credentials) and resolves `--workspace` from the
//! directory it runs in, and a lockfile sits beside the workspace root. If publishing, observing and refreshing each
//! guessed that directory their own way they could disagree about it, so they all ask here. The owner is the nearest
//! directory at or above the package whose manifest declares a workspace, the same declaration discovery reads; only for
//! a package no workspace declares does a lockfile or marker file decide.

use std::path::{Path, PathBuf};

use crate::error::GraphError;
use crate::locate::{declares_npm_workspace, declares_python_workspace};

/// Files that mark a directory as the root of an npm-family project even though no workspace is declared in it.
pub(crate) const NPM_LOCKFILES: [&str; 5] = [
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
];
pub(crate) const PYPI_LOCKFILES: [&str; 4] = ["uv.lock", "poetry.lock", "pdm.lock", "Pipfile.lock"];

/// The npm workspace directory, relative to `root`, that owns the package at `package_dir`.
///
/// # Errors
///
/// A manifest or marker on the way up cannot be read or understood.
pub(crate) fn npm_workspace_dir(root: &Path, package_dir: &Path) -> Result<PathBuf, GraphError> {
    let markers: Vec<&str> = NPM_LOCKFILES.iter().copied().chain(["pnpm-workspace.yaml"]).collect();
    owner(root, package_dir, declares_npm_workspace, &markers)
}

/// The Python (uv) workspace directory, relative to `root`, that owns the package at `package_dir`.
///
/// # Errors
///
/// A manifest or marker on the way up cannot be read or understood.
pub(crate) fn python_workspace_dir(root: &Path, package_dir: &Path) -> Result<PathBuf, GraphError> {
    owner(root, package_dir, declares_python_workspace, &PYPI_LOCKFILES)
}

fn owner(
    root: &Path,
    package_dir: &Path,
    declares: impl Fn(&Path) -> Result<bool, crate::locate::LocateError>,
    markers: &[&str],
) -> Result<PathBuf, GraphError> {
    let dirs: Vec<&Path> = package_dir.ancestors().collect();
    for dir in &dirs {
        if declares(&root.join(dir))? {
            return Ok(relative(dir));
        }
    }
    for dir in &dirs {
        for marker in markers {
            let path = root.join(dir).join(marker);
            let present = callisto_model::strict_fs::exists(&path).map_err(|error| GraphError::ReleaseInputRead {
                path: path.clone(),
                message: error.to_string(),
            })?;
            if present {
                return Ok(relative(dir));
            }
        }
    }
    Ok(PathBuf::new())
}

/// `.` is the root, spelled as an empty path like everywhere else a directory is relative to it.
fn relative(dir: &Path) -> PathBuf {
    if dir == Path::new(".") {
        PathBuf::new()
    } else {
        dir.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_declared_workspace_owns_its_packages_with_no_lockfile_anywhere() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "package.json", r#"{"name":"marker","private":true}"#);
        write(
            dir.path(),
            "web/package.json",
            r#"{"name":"web","private":true,"workspaces":["packages/*"]}"#,
        );
        write(
            dir.path(),
            "web/packages/a/package.json",
            r#"{"name":"a","version":"1.0.0"}"#,
        );
        assert_eq!(
            npm_workspace_dir(dir.path(), Path::new("web/packages/a")).unwrap(),
            Path::new("web")
        );
    }

    /// A lockfile beside the package is a lockfile, not a workspace: the declaring directory above it still owns it.
    #[test]
    fn a_lockfile_beside_a_package_does_not_take_it_out_of_its_declared_workspace() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "web/package.json",
            r#"{"name":"web","private":true,"workspaces":["packages/*"]}"#,
        );
        write(
            dir.path(),
            "web/packages/a/package.json",
            r#"{"name":"a","version":"1.0.0"}"#,
        );
        write(dir.path(), "web/packages/a/package-lock.json", "{}");
        assert_eq!(
            npm_workspace_dir(dir.path(), Path::new("web/packages/a")).unwrap(),
            Path::new("web")
        );
    }

    #[test]
    fn pnpm_workspace_yaml_declares_one_and_a_lockfile_decides_only_when_nothing_is_declared() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "apps/pnpm-workspace.yaml", "packages:\n  - 'x/*'\n");
        write(dir.path(), "apps/x/a/package.json", r#"{"name":"a","version":"1.0.0"}"#);
        write(dir.path(), "solo/yarn.lock", "");
        write(dir.path(), "solo/pkg/package.json", r#"{"name":"b","version":"1.0.0"}"#);
        write(dir.path(), "bare/pkg/package.json", r#"{"name":"c","version":"1.0.0"}"#);
        assert_eq!(
            npm_workspace_dir(dir.path(), Path::new("apps/x/a")).unwrap(),
            Path::new("apps")
        );
        assert_eq!(
            npm_workspace_dir(dir.path(), Path::new("solo/pkg")).unwrap(),
            Path::new("solo")
        );
        assert_eq!(
            npm_workspace_dir(dir.path(), Path::new("bare/pkg")).unwrap(),
            Path::new("")
        );
        assert_eq!(npm_workspace_dir(dir.path(), Path::new(".")).unwrap(), Path::new(""));
    }

    #[test]
    fn a_declared_uv_workspace_owns_its_members() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "py/pyproject.toml",
            "[tool.uv.workspace]\nmembers = [\"libs/*\"]\n",
        );
        write(
            dir.path(),
            "py/libs/a/pyproject.toml",
            "[project]\nname = \"a\"\nversion = \"1.0.0\"\n",
        );
        assert_eq!(
            python_workspace_dir(dir.path(), Path::new("py/libs/a")).unwrap(),
            Path::new("py")
        );
    }
}
