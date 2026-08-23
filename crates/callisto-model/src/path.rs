use std::path::{Component, Path, PathBuf};

use camino::Utf8PathBuf;
use path_slash::PathExt as _;

use crate::ModelError;

/// Normalizes a path to be workspace-root-relative and UTF-8.
///
/// Rejects absolute paths and non-UTF-8 paths, and normalizes `.` and `..` components
/// lexically (without accessing the filesystem).
///
/// Separator normalization is platform-native, via `path_slash::PathExt::to_slash`: on
/// Windows, `\` (the native separator) is converted to `/`; on POSIX, `/` is already
/// native, so a literal `\` in a real filename is preserved as-is rather than being
/// mistaken for a separator and corrupting the path. The previous unconditional
/// `.replace('\\', "/")` treated `\` as a separator on every platform, which was
/// byte-incorrect on POSIX (where `\` is a legal filename character).
///
/// **Caveat for callers parsing a user-authored config string** (as opposed to a path this
/// process discovered on its own local filesystem): "platform-native" means the platform
/// *running this call*, not the platform the string was *written* on. `callisto.toml` is
/// checked into version control and read on whatever OS a given developer or CI runner
/// happens to be using, so a `\`-separated value authored on Windows normalizes correctly
/// there but is read back as one literal, oddly-named path component the moment the same
/// file is read on POSIX. Callers taking a config value (e.g. `[changesets].dir`,
/// `[[package]] changelog`) must document that the value should always use `/`, regardless
/// of authoring platform — this function does not and cannot enforce that on their behalf.
pub fn workspace_relative(path: impl AsRef<Path>) -> Result<PathBuf, ModelError> {
    let p = path.as_ref();
    if p.is_absolute() {
        return Err(ModelError::AbsolutePath { path: p.to_path_buf() });
    }

    let utf8 = Utf8PathBuf::from_path_buf(p.to_path_buf()).map_err(|path| ModelError::NonUtf8Path { path })?;

    // `to_slash()` can only return `None` for non-UTF-8 input, which `utf8` has
    // already ruled out -- unreachable in practice, kept as a typed error path
    // rather than a panic in case that invariant ever changes upstream.
    let slash_str = utf8.as_std_path().to_slash().ok_or_else(|| ModelError::NonUtf8Path {
        path: utf8.as_std_path().to_path_buf(),
    })?;
    let normalized_path = Path::new(slash_str.as_ref());

    let mut components = Vec::new();
    for component in normalized_path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(ModelError::AbsolutePath { path: p.to_path_buf() });
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if let Some(Component::Normal(_)) = components.last() {
                    components.pop();
                } else {
                    return Err(ModelError::PathTraversal { path: p.to_path_buf() });
                }
            }
            Component::Normal(_) => {
                components.push(component);
            }
        }
    }

    let mut out_str = String::new();
    for (i, c) in components.iter().enumerate() {
        if i > 0 {
            out_str.push('/');
        }
        out_str.push_str(c.as_os_str().to_str().unwrap());
    }

    Ok(PathBuf::from(out_str))
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

    /// On Windows, `\` is the native path separator, so `path_slash::to_slash`
    /// converts it -- a path authored with Windows-style separators still
    /// normalizes to POSIX-style output.
    #[cfg(target_os = "windows")]
    #[test]
    fn normalizes_windows_backslashes_to_posix_slashes() {
        let p = workspace_relative("crates\\callisto-cli\\src\\lib.rs").unwrap();
        assert_eq!(p, PathBuf::from("crates/callisto-cli/src/lib.rs"));
    }

    /// Regression: on POSIX, `\` is a legal filename character, not a path
    /// separator. The previous unconditional `.replace('\\', "/")` treated
    /// every backslash as a separator regardless of platform, silently
    /// corrupting a real POSIX filename containing one (e.g. splitting
    /// `weird\file.txt` into two path components). `path_slash::to_slash`
    /// is a no-op on POSIX, so the literal backslash must survive intact.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn preserves_literal_backslash_in_posix_filename() {
        let p = workspace_relative("a/weird\\file.txt").unwrap();
        assert_eq!(p, PathBuf::from("a/weird\\file.txt"));
    }
}
