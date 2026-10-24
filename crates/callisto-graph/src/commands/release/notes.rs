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

/// A fixed-group member's changelog line that only says it was released with the group.
fn is_group_boilerplate(entry: &str) -> bool {
    entry.starts_with("- Released together with the `") || entry.starts_with("- Joined the `")
}

/// The changelog entries of a section: each `- ` bullet with its indented continuation lines.
fn entries(section: &str) -> Vec<String> {
    let mut entries: Vec<String> = Vec::new();
    for line in section.lines() {
        match entries.last_mut() {
            Some(last) if line.starts_with(' ') || line.is_empty() => {
                last.push('\n');
                last.push_str(line);
            }
            _ if line.starts_with("- ") => entries.push(line.to_owned()),
            _ => {}
        }
    }
    entries.iter().map(|entry| entry.trim_end().to_owned()).collect()
}

/// Release notes for a package in a fixed group: its own changelog section, then the entries that the group's other
/// members recorded for the same version, each marked with the member it came from. The group is released as one
/// unit, so a change recorded against a member that owns no release still belongs in the release's notes. An entry
/// that is only the group boilerplate, or that the notes already hold, is left out. With no entry from another member
/// the notes are exactly [`release_notes`].
///
/// # Errors
///
/// A changelog exists but cannot be read.
pub(crate) fn group_release_notes(
    root: &Path,
    changelog: Option<&Path>,
    members: &[(String, &Path)],
    version: &Version,
) -> Result<ReleaseNotes, GraphError> {
    let own = release_notes(root, changelog, version)?;
    let text = match &own {
        ReleaseNotes::Section(section) => section.clone(),
        ReleaseNotes::Generated { .. } => String::new(),
    };
    let mut extra = Vec::new();
    for (name, member_changelog) in members {
        let Ok(section) = changelog_section(root, member_changelog, version)? else {
            continue;
        };
        for entry in entries(&section) {
            let Some(body) = entry.strip_prefix("- ") else {
                continue;
            };
            if is_group_boilerplate(&entry)
                || text.contains(body)
                || extra.iter().any(|seen: &String| seen.contains(body))
            {
                continue;
            }
            extra.push(format!("- `{name}`: {body}"));
        }
    }
    if extra.is_empty() {
        return Ok(own);
    }
    let mut kept: Vec<String> = entries(&text)
        .into_iter()
        .filter(|entry| !is_group_boilerplate(entry))
        .collect();
    kept.extend(extra);
    Ok(ReleaseNotes::Section(kept.join("\n")))
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

    /// `files` are `(name, changelog)` pairs; the first is the release owner, the rest its fixed-group mates.
    fn group_notes(files: &[(&str, Option<&str>)]) -> ReleaseNotes {
        let dir = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for (name, body) in files {
            let path = std::path::PathBuf::from(format!("{}.md", name.replace('/', "-")));
            if let Some(body) = body {
                std::fs::write(dir.path().join(&path), body).unwrap();
            }
            paths.push(((*name).to_owned(), path));
        }
        let version = Version::parse("1.0.0", VersionGrammar::SemVer).unwrap();
        let (_, owner) = &paths[0];
        let mates: Vec<(String, &Path)> = paths[1..].iter().map(|(n, p)| (n.clone(), p.as_path())).collect();
        group_release_notes(dir.path(), Some(owner), &mates, &version).unwrap()
    }

    const BOILERPLATE: &str = "## 1.0.0\n\n- Released together with the `lockstep` fixed group.\n";

    /// The owner records only that it was released with the group, while the change was recorded against a mate.
    #[test]
    fn a_mates_entries_reach_the_owners_release_notes() {
        let notes = group_notes(&[
            ("cargo/a", Some(BOILERPLATE)),
            ("cargo/b", Some("## 1.0.0\n\n- B gets a feature.\n")),
        ]);
        assert_eq!(
            notes,
            ReleaseNotes::Section("- `cargo/b`: B gets a feature.".to_owned())
        );
    }

    #[test]
    fn the_owners_own_entries_come_first_and_a_shared_entry_is_listed_once() {
        let notes = group_notes(&[
            ("cargo/a", Some("## 1.0.0\n\n- Shared change.\n- A only.\n")),
            (
                "cargo/b",
                Some("## 1.0.0\n\n- Shared change.\n- B only.\n- Joined the `lockstep` group at this version.\n"),
            ),
            ("cargo/c", Some("## 1.0.0\n\n- B only.\n")),
        ]);
        assert_eq!(
            notes,
            ReleaseNotes::Section("- Shared change.\n- A only.\n- `cargo/b`: B only.".to_owned())
        );
    }

    #[test]
    fn a_multi_line_entry_keeps_its_continuation_lines() {
        let notes = group_notes(&[
            ("cargo/a", Some(BOILERPLATE)),
            ("cargo/b", Some("## 1.0.0\n\n- Big change.\n  with detail.\n")),
        ]);
        assert_eq!(
            notes,
            ReleaseNotes::Section("- `cargo/b`: Big change.\n  with detail.".to_owned())
        );
    }

    /// With nothing to add from a mate, the notes are what they were before groups were considered.
    #[test]
    fn a_group_with_nothing_to_add_leaves_the_owners_notes_alone() {
        let own = Some("## 1.0.0\n\n- A change.\n- Released together with the `lockstep` fixed group.\n");
        assert_eq!(
            group_notes(&[("cargo/a", own), ("cargo/b", Some(BOILERPLATE))]),
            ReleaseNotes::Section("- A change.\n- Released together with the `lockstep` fixed group.".to_owned())
        );
        assert_eq!(
            group_notes(&[("cargo/a", None), ("cargo/b", Some(BOILERPLATE))]),
            generated(NotesFallback::FileMissing)
        );
    }

    /// An owner with no changelog section still gets the group's entries rather than generated notes.
    #[test]
    fn an_owner_without_a_section_takes_the_mates_entries() {
        let notes = group_notes(&[
            ("cargo/a", None),
            ("cargo/b", Some("## 1.0.0\n\n- B gets a feature.\n")),
        ]);
        assert_eq!(
            notes,
            ReleaseNotes::Section("- `cargo/b`: B gets a feature.".to_owned())
        );
    }
}
