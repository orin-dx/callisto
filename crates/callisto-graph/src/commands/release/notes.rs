//! The one changelog-section reader behind release notes.

use std::path::Path;

use callisto_model::Version;

use super::provider::{NotesFallback, ReleaseNotes};

/// The non-empty `## {version}` section of the changelog at `root.join(changelog)`.
fn changelog_section(root: &Path, changelog: &Path, version: &Version) -> Result<String, NotesFallback> {
    let content = match std::fs::read_to_string(root.join(changelog)) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(NotesFallback::FileMissing),
        Err(_) => return Err(NotesFallback::Unreadable),
    };
    match callisto_changelog::find_section(&content, version) {
        None => Err(NotesFallback::SectionMissing),
        Some("") => Err(NotesFallback::SectionEmpty),
        Some(section) => Ok(section.to_owned()),
    }
}

/// Release notes for a package: its changelog section, else generated notes and why.
pub(crate) fn release_notes(root: &Path, changelog: Option<&Path>, version: &Version) -> ReleaseNotes {
    let Some(changelog) = changelog else {
        return ReleaseNotes::Generated {
            reason: NotesFallback::NotConfigured,
        };
    };
    match changelog_section(root, changelog, version) {
        Ok(section) => ReleaseNotes::Section(section),
        Err(reason) => ReleaseNotes::Generated { reason },
    }
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
        release_notes(dir.path(), changelog.map(Path::new), &version)
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

    /// A changelog that exists but cannot be read falls back as `Unreadable`.
    #[test]
    fn unreadable_changelog_falls_back_to_generated_notes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("CHANGELOG.md")).unwrap();
        let version = Version::parse("1.0.0", VersionGrammar::SemVer).unwrap();
        assert_eq!(
            release_notes(dir.path(), Some(Path::new("CHANGELOG.md")), &version),
            generated(NotesFallback::Unreadable)
        );
    }
}
