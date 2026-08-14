use std::fs;
use std::path::{Path, PathBuf};

use crate::locate::LocateError;

pub fn find_workspace_root(start: &Path) -> Result<PathBuf, LocateError> {
    let canonical = dunce::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let mut current: &Path = &canonical;

    loop {
        if is_workspace_root(current) {
            return Ok(current.to_path_buf());
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => {
                return Err(LocateError::WorkspaceRootNotFound {
                    start: start.to_path_buf(),
                });
            }
        }
    }
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
