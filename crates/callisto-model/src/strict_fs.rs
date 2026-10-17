//! Reads that tell "not there" from "could not look".
//!
//! `Path::exists` and `read_to_string(..).ok()` answer "absent" for a permission error, invalid UTF-8 or a loop. Code
//! that decides what a release contains must not take those for a missing file.

use std::io;
use std::path::Path;

/// The text of `path`, or `None` only when it does not exist. A permission error, a directory or invalid UTF-8 is an error.
///
/// # Errors
///
/// Any I/O error other than the file being absent.
pub fn read_optional(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if is_absent(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Nothing at the path, or a regular file where a parent directory should be: either way the path holds nothing.
fn is_absent(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory)
}

/// Whether `path` exists. A parent that cannot be searched is an error, not `false`.
///
/// # Errors
///
/// Any I/O error other than the path being absent.
pub fn exists(path: &Path) -> io::Result<bool> {
    match path.try_exists() {
        Err(error) if is_absent(&error) => Ok(false),
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_none_and_present_text_is_returned() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.toml");
        assert_eq!(read_optional(&file).unwrap(), None);
        assert!(!exists(&file).unwrap());
        std::fs::write(&file, "x = 1\n").unwrap();
        assert_eq!(read_optional(&file).unwrap().as_deref(), Some("x = 1\n"));
        assert!(exists(&file).unwrap());
    }

    #[test]
    fn a_path_beneath_a_regular_file_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(read_optional(&file.join("inner.toml")).unwrap(), None);
        assert!(!exists(&file.join("inner.toml")).unwrap());
    }

    #[test]
    fn invalid_utf8_and_a_directory_are_errors_not_absence() {
        let dir = tempfile::tempdir().unwrap();
        let latin1 = dir.path().join("Cargo.toml");
        std::fs::write(&latin1, b"name = \"caf\xe9\"").unwrap();
        assert_eq!(read_optional(&latin1).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(read_optional(dir.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_or_unsearchable_parent_is_an_error_not_absence() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        let file = locked.join("pre.json");
        std::fs::write(&file, "{}").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root bypasses permission bits, so only assert where the directory really is locked.
        let locked_out = std::fs::read_to_string(&file).is_err();
        let read = read_optional(&file);
        let present = exists(&file);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if locked_out {
            assert!(read.is_err(), "{read:?}");
            assert!(present.is_err(), "{present:?}");
        }
    }
}
