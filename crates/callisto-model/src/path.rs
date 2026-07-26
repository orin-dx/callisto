use std::path::{Component, Path, PathBuf};

use crate::ModelError;

/// Normalizes a path to be workspace-root-relative and UTF-8.
///
/// Rejects absolute paths and non-UTF-8 paths, and normalizes `.` and `..` components
/// lexically (without accessing the filesystem).
pub fn workspace_relative(path: impl AsRef<Path>) -> Result<PathBuf, ModelError> {
    let p = path.as_ref();
    if p.is_absolute() {
        return Err(ModelError::AbsolutePath {
            path: p.to_path_buf(),
        });
    }

    if p.to_str().is_none() {
        return Err(ModelError::NonUtf8Path {
            path: p.to_path_buf(),
        });
    }

    let mut components = Vec::new();
    for component in p.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(ModelError::AbsolutePath {
                    path: p.to_path_buf(),
                });
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if let Some(Component::Normal(_)) = components.last() {
                    components.pop();
                } else {
                    return Err(ModelError::PathTraversal {
                        path: p.to_path_buf(),
                    });
                }
            }
            Component::Normal(_) => {
                components.push(component);
            }
        }
    }

    let mut out = PathBuf::new();
    for c in components {
        out.push(c.as_os_str());
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_relative_paths_lexically() {
        let p = workspace_relative("a/b/../c/./d").unwrap();
        assert_eq!(p, PathBuf::from("a/c/d"));
    }

    #[test]
    fn rejects_absolute_path() {
        let err = workspace_relative("/usr/bin").unwrap_err();
        assert!(matches!(err, ModelError::AbsolutePath { .. }));
    }

    #[test]
    fn rejects_path_traversal_outside_workspace() {
        let err = workspace_relative("../secret").unwrap_err();
        assert!(matches!(err, ModelError::PathTraversal { .. }));
    }
}
