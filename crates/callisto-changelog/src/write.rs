use std::fs;
use std::path::Path;

use callisto_model::{ApplyPermit, Version};

use crate::ChangelogError;

/// Prepend a rendered changelog entry for `display_name` to the
/// changelog file at `changelog_path` (relative to `root`).
///
/// # Concurrency
///
/// **Not safe for concurrent calls on the same `path`.** Reads existing
/// content then writes atomically, but a second concurrent call could
/// read stale content between those two points. Callers must ensure
/// sequential access per changelog file -- if this ever moves to
/// parallel `apply`, add a per-path lock before removing this note.
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

    // Normalise CRLF → LF so the prefix patterns always match, then restore
    // the original line endings on the way out if the file used CRLF.
    let had_crlf = existing.contains("\r\n");
    let normalised = if had_crlf {
        existing.replace("\r\n", "\n")
    } else {
        existing.clone()
    };

    // Bug 8 guard: if the version heading we are about to insert already exists
    // in the file, skip the write entirely (idempotent behaviour). The heading
    // is the first line of `rendered` (e.g. "## 1.0.0").
    let version_heading = rendered.lines().next().unwrap_or("").trim();
    if !version_heading.is_empty() && normalised.lines().any(|l| l.trim() == version_heading) {
        return Ok(());
    }

    let mut new_content = String::new();
    if let Some(rest) = normalised.strip_prefix(&format!("# {display_name}\n\n")) {
        new_content.push_str(&format!("# {display_name}\n\n"));
        new_content.push_str(rendered);
        if !rendered.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push('\n');
        new_content.push_str(rest);
    } else if let Some(rest) = normalised.strip_prefix(&format!("# {display_name}\n")) {
        new_content.push_str(&format!("# {display_name}\n\n"));
        new_content.push_str(rendered);
        if !rendered.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push('\n');
        new_content.push_str(rest);
    } else {
        // The existing header didn't match display_name (e.g. casing change,
        // HTML anchor, or package rename). Always emit the correct H1 first so
        // the output is well-formed: H1 → new entry → existing body.
        new_content.push_str(&format!("# {display_name}\n\n"));
        new_content.push_str(rendered);
        if !rendered.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push('\n');
        new_content.push_str(&normalised);
    }

    if had_crlf {
        new_content = new_content.replace('\n', "\r\n");
    }

    callisto_model::atomic::atomic_write(&full_path, &new_content, permit).map_err(|e| ChangelogError::WriteFailed {
        path: changelog_path.to_path_buf(),
        message: e.to_string(),
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
    /// `callisto_model::atomic::atomic_write` does and the local copy did not.
    ///
    /// This test doesn't simulate a crash (that would require fault injection at
    /// the syscall level), but it does exercise `prepend()` through a freshly
    /// created two-level-deep directory tree end-to-end, proving the write path
    /// now routes through the shared implementation rather than a local
    /// duplicate that silently drops the grandparent sync.
    #[test]
    fn prepend_does_not_displace_header_in_crlf_changelog() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let changelog_path = std::path::Path::new("CHANGELOG.md");
        let full_path = root.join(changelog_path);

        // Write an existing CRLF changelog.
        let existing_crlf = "# My Package\r\n\r\n## 1.0.0\r\n\r\nSome content\r\n";
        fs::write(&full_path, existing_crlf).unwrap();

        prepend(root, changelog_path, "My Package", "## 1.1.0\n\nNew stuff\n", &permit())
            .expect("prepend should succeed on a CRLF changelog");

        let result = fs::read_to_string(&full_path).unwrap();

        // The header must remain the very first thing in the file.
        assert!(
            result.starts_with("# My Package"),
            "header was displaced — got:\n{result}"
        );

        // The new section must appear before the old section.
        let pos_new = result.find("1.1.0").expect("new version missing");
        let pos_old = result.find("1.0.0").expect("old version missing");
        assert!(pos_new < pos_old, "new entry should precede old entry — got:\n{result}");
    }

    #[test]
    fn prepend_else_branch_inserts_h1_header_before_new_entry_when_display_name_mismatches() {
        // The existing changelog uses "old-name" as the H1. prepend() is called
        // with display_name = "new-name", so neither strip_prefix pattern matches
        // and the else branch fires. Before the fix, the else branch emits the
        // rendered entry (an H2) first, leaving the H1 buried below it.
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let changelog_path = std::path::Path::new("CHANGELOG.md");
        let full_path = root.join(changelog_path);

        let existing = "# old-name\n\n## 1.0.0\n\nOld stuff\n";
        fs::write(&full_path, existing).unwrap();

        prepend(
            root,
            changelog_path,
            "new-name",
            "## 2.0.0\n\n### Patch Changes\n\n- Something\n",
            &permit(),
        )
        .expect("prepend should succeed");

        let result = fs::read_to_string(&full_path).unwrap();

        assert!(
            result.starts_with("# new-name\n\n"),
            "result should start with H1 heading, got:\n{result}"
        );
        assert!(
            !result.starts_with("## 2.0.0"),
            "result must not start with H2 (no H1 at top), got:\n{result}"
        );
        let pos_new = result.find("## 2.0.0").expect("new version heading missing");
        let pos_old = result.find("## 1.0.0").expect("old version heading missing");
        assert!(pos_new < pos_old, "new entry should precede old entry — got:\n{result}");
    }

    #[test]
    fn prepend_is_idempotent_for_same_version() {
        // If the existing changelog already contains the version heading being
        // inserted, prepend() should return the existing content unchanged
        // rather than duplicating the section.
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let changelog_path = std::path::Path::new("CHANGELOG.md");
        let full_path = root.join(changelog_path);

        let existing = "# my-pkg\n\n## 1.0.0\n\nSome content\n";
        fs::write(&full_path, existing).unwrap();

        prepend(root, changelog_path, "my-pkg", "## 1.0.0\n\nNew content\n", &permit())
            .expect("prepend should succeed");

        let result = fs::read_to_string(&full_path).unwrap();

        let occurrences = result.matches("## 1.0.0").count();
        assert_eq!(
            occurrences, 1,
            "## 1.0.0 should appear exactly once, got {occurrences} times in:\n{result}"
        );
        assert!(
            result.contains("Some content"),
            "existing content should be preserved — got:\n{result}"
        );
    }

    #[test]
    fn test_prepend_stable_after_prerelease_not_suppressed() {
        // CORR-001: "## 1.0.0" must not be suppressed by the idempotency guard
        // when the file already contains "## 1.0.0-alpha.1", because the stable
        // version string is a substring of the pre-release heading.
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let changelog_path = std::path::Path::new("CHANGELOG.md");
        let full_path = root.join(changelog_path);

        let existing = "## 1.0.0-alpha.1\n\n- alpha change\n";
        fs::write(&full_path, existing).unwrap();

        prepend(
            root,
            changelog_path,
            "my-pkg",
            "## 1.0.0\n\n### Patch Changes\n\n- Stable release\n",
            &permit(),
        )
        .expect("prepend should succeed");

        let result = fs::read_to_string(&full_path).unwrap();

        // The stable heading must appear as a standalone line, not just as
        // a substring match that could fire on "## 1.0.0-alpha.1".
        assert!(
            result.lines().any(|l| l.trim() == "## 1.0.0"),
            "stable heading '## 1.0.0' must appear as its own line; got:\n{result}"
        );
        // The pre-release heading must still be present.
        assert!(
            result.contains("## 1.0.0-alpha.1"),
            "pre-release heading must still be present; got:\n{result}"
        );
    }

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
        assert_eq!(parent_entries, vec![std::ffi::OsString::from("CHANGELOG.md")]);
    }
}
