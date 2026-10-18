//! The one changelog-section reader behind release notes.

use std::path::Path;

use callisto_model::Version;

use crate::error::GraphError;

use super::provider::{NotesFallback, ReleaseNotes};

/// The non-empty `## {version}` section of the changelog at `root.join(changelog)`, or why there is none. A changelog
/// that is there but cannot be read is an error, not a reason: the notes would silently differ from what was reviewed.
fn changelog_section(
    root: &Path,
    changelog: &Path,
    version: &Version,
) -> Result<Result<String, NotesFallback>, GraphError> {
    let path = root.join(changelog);
    let content = match callisto_model::strict_fs::read_optional(&path) {
        Ok(Some(content)) => content,
        Ok(None) => return Ok(Err(NotesFallback::FileMissing)),
        Err(error) => {
            return Err(GraphError::ReleaseInputRead {
                path,
                message: error.to_string(),
            })
        }
    };
    Ok(match crate::changelog::find_section(&content, version) {
        None => Err(NotesFallback::SectionMissing),
        Some("") => Err(NotesFallback::SectionEmpty),
        Some(section) => Ok(section.to_owned()),
    })
}

/// Release notes for a package: its changelog section, else generated notes and why.
///
/// # Errors
///
/// The changelog exists but cannot be read.
pub(crate) fn release_notes(
    root: &Path,
    changelog: Option<&Path>,
    version: &Version,
) -> Result<ReleaseNotes, GraphError> {
    let Some(changelog) = changelog else {
        return Ok(ReleaseNotes::Generated {
            reason: NotesFallback::NotConfigured,
        });
    };
    Ok(match changelog_section(root, changelog, version)? {
        Ok(section) => ReleaseNotes::Section(section),
        Err(reason) => ReleaseNotes::Generated { reason },
    })
}

#[cfg(test)]
mod tests {
    use callisto_model::VersionGrammar;

    use super::*;

    fn notes(changelog: Option<&str>, body: Option<&str>) -> ReleaseNotes {
        let dir = tempfile::tempdir().unwrap();
        if let Some(body) = body {
            std::fs::write(dir.path().join("CHANGELOG.md"), body).unwrap();
        }
        let version = Version::parse("1.0.0", VersionGrammar::SemVer).unwrap();
        release_notes(dir.path(), changelog.map(Path::new), &version).unwrap()
    }

    fn generated(reason: NotesFallback) -> ReleaseNotes {
        ReleaseNotes::Generated { reason }
    }

    #[test]
    fn the_version_section_becomes_the_release_notes() {
        assert_eq!(
            notes(Some("CHANGELOG.md"), Some("## 1.0.0\n\n- fix\n\n## 0.9.0\n\n- old\n")),
            ReleaseNotes::Section("- fix".to_owned())
        );
    }

    /// A missing file, a missing heading, or an empty section falls back, never fails.
    #[test]
    fn missing_file_heading_or_content_falls_back_to_generated_notes() {
        assert_eq!(notes(Some("CHANGELOG.md"), None), generated(NotesFallback::FileMissing));
        assert_eq!(
            notes(Some("CHANGELOG.md"), Some("## 0.9.0\n\n- old\n")),
            generated(NotesFallback::SectionMissing)
        );
        assert_eq!(
            notes(Some("CHANGELOG.md"), Some("## 1.0.0\n\n## 0.9.0\n\n- old\n")),
            generated(NotesFallback::SectionEmpty)
        );
        assert_eq!(notes(None, None), generated(NotesFallback::NotConfigured));
    }

    /// A changelog that exists but cannot be read (a directory, bytes that are not UTF-8) fails planning, not falls
    /// back: generated notes would replace what the release PR reviewed without anyone choosing that.
    #[test]
    fn an_unreadable_changelog_is_an_error_not_generated_notes() {
        let version = Version::parse("1.0.0", VersionGrammar::SemVer).unwrap();
        for unreadable in ["directory", "not utf-8"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("CHANGELOG.md");
            if unreadable == "directory" {
                std::fs::create_dir(&path).unwrap();
            } else {
                std::fs::write(&path, b"## 1.0.0\n\n- caf\xe9\n").unwrap();
            }
            let error = release_notes(dir.path(), Some(Path::new("CHANGELOG.md")), &version).unwrap_err();
            assert!(
                matches!(&error, GraphError::ReleaseInputRead { path, .. } if path.ends_with("CHANGELOG.md")),
                "{unreadable}: {error:?}"
            );
        }
    }
}
