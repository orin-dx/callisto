use std::fs;
use std::path::Path;

use callisto_model::{ApplyPermit, Version};

use crate::ChangelogError;

pub fn prepend(
    root: &Path,
    changelog_path: &Path,
    display_name: &str,
    rendered: &str,
    permit: &ApplyPermit,
) -> Result<(), ChangelogError> {
    let full_path = root.join(changelog_path);
    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent).map_err(|e| ChangelogError::WriteFailed {
            path: changelog_path.to_path_buf(),
            message: e.to_string(),
        })?;
    }

    let existing = if full_path.exists() {
        fs::read_to_string(&full_path).map_err(|e| ChangelogError::ReadFailed {
            path: changelog_path.to_path_buf(),
            message: e.to_string(),
        })?
    } else {
        format!("# {display_name}\n\n")
    };

    let mut new_content = String::new();
    if let Some(rest) = existing.strip_prefix(&format!("# {display_name}\n\n")) {
        new_content.push_str(&format!("# {display_name}\n\n"));
        new_content.push_str(rendered);
        if !rendered.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push('\n');
        new_content.push_str(rest);
    } else if let Some(rest) = existing.strip_prefix(&format!("# {display_name}\n")) {
        new_content.push_str(&format!("# {display_name}\n\n"));
        new_content.push_str(rendered);
        if !rendered.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push('\n');
        new_content.push_str(rest);
    } else {
        new_content.push_str(rendered);
        if !rendered.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push('\n');
        new_content.push_str(&existing);
    }

    callisto_manifests::atomic::atomic_write(&full_path, &new_content, permit).map_err(|e| {
        ChangelogError::WriteFailed {
            path: changelog_path.to_path_buf(),
            message: e.to_string(),
        }
    })
}

pub fn extract_section<'a>(changelog: &'a str, version: &Version) -> Option<&'a str> {
    let target_heading = format!("## {}", version.render());
    let mut start_byte = None;

    let mut offset = 0;
    for line in changelog.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if trimmed == target_heading {
            start_byte = Some(offset + line.len());
            break;
        }
        offset += line.len();
    }

    let start = start_byte?;
    let slice_after = &changelog[start.min(changelog.len())..];

    let mut end = slice_after.len();
    let mut line_offset = 0;

    for line in slice_after.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if line_offset > 0 && (trimmed.starts_with("## ") || trimmed.starts_with("# ")) {
            end = line_offset;
            break;
        }
        line_offset += line.len();
    }

    let raw_section = slice_after[..end.min(slice_after.len())].trim();
    if raw_section.is_empty() {
        None
    } else {
        Some(raw_section)
    }
}

#[cfg(test)]
mod tests {

    /// Tests exercise the write primitives directly rather than through a
    /// command handler, so they mint a permit without a dry-run flag to
    /// consult. Every non-test caller must go through
    /// `ApplyPermit::granted_unless_dry_run`.
    fn permit() -> callisto_model::ApplyPermit {
        callisto_model::ApplyPermit::force_for_tests()
    }
    use super::*;
    use callisto_model::VersionGrammar;

    #[test]
    fn extracts_rendered_section() {
        let changelog = r#"# my-pkg

## 1.1.0

### Minor Changes

- Add cool feature

## 1.0.0

### Patch Changes

- Initial release
"#;
        let v1_1 = Version::parse("1.1.0", VersionGrammar::SemVer).unwrap();
        let extracted = extract_section(changelog, &v1_1).unwrap();
        assert!(extracted.contains("### Minor Changes"));
        assert!(extracted.contains("- Add cool feature"));
        assert!(!extracted.contains("## 1.0.0"));
    }

    #[test]
    fn extracts_crlf_changelog_section() {
        let changelog = "# my-pkg\r\n\r\n## 1.1.0\r\n\r\n### Minor Changes\r\n\r\n- Feature\r\n\r\n## 1.0.0\r\n";
        let v1_1 = Version::parse("1.1.0", VersionGrammar::SemVer).unwrap();
        let extracted = extract_section(changelog, &v1_1).unwrap();
        assert!(extracted.contains("### Minor Changes"));
        assert!(extracted.contains("- Feature"));
    }

    /// Regression test for the drifted local `atomic_write` that only synced the
    /// parent directory. `prepend()` unconditionally calls `create_dir_all` for
    /// the changelog's parent, so when both the parent *and* grandparent
    /// directories are freshly created as part of the same operation, durability
    /// requires fsyncing both new directory entries — exactly what the canonical
    /// `callisto_manifests::atomic::atomic_write` does and the local copy did not.
    ///
    /// This test doesn't simulate a crash (that would require fault injection at
    /// the syscall level), but it does exercise `prepend()` through a freshly
    /// created two-level-deep directory tree end-to-end, proving the write path
    /// now routes through the shared implementation rather than a local
    /// duplicate that silently drops the grandparent sync.
    #[test]
    fn prepend_creates_nested_grandparent_and_parent_dirs_via_shared_atomic_write() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();

        // Neither "packages" (grandparent) nor "packages/my-pkg" (parent) exist yet.
        let changelog_path = std::path::Path::new("packages/my-pkg/CHANGELOG.md");
        assert!(!root.join("packages").exists());

        prepend(
            root,
            changelog_path,
            "my-pkg",
            "### Patch Changes\n\n- Fix bug",
            &permit(),
        )
        .expect("prepend should create nested dirs and write durably");

        let full_path = root.join(changelog_path);
        assert!(full_path.exists());
        let contents = fs::read_to_string(&full_path).unwrap();
        assert!(contents.starts_with("# my-pkg\n\n"));
        assert!(contents.contains("- Fix bug"));

        // No leftover temp files from the shared NamedTempFile-based atomic_write.
        let parent_entries: Vec<_> = fs::read_dir(full_path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            parent_entries,
            vec![std::ffi::OsString::from("CHANGELOG.md")]
        );
    }
}
