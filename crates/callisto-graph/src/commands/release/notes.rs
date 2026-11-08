//! The one changelog-section reader behind release notes.

use std::collections::HashSet;
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

/// The bump groups a grouped changelog section is split into, highest first.
const GROUP_HEADINGS: [&str; 3] = ["### Major Changes", "### Minor Changes", "### Patch Changes"];

fn group_of(line: &str) -> Option<usize> {
    GROUP_HEADINGS.iter().position(|heading| *heading == line)
}

/// For each line of `text`, the bump group it opens. A group heading inside fenced code is an example, not a boundary,
/// so the shared fence tracker decides which lines count.
fn group_openers(text: &str) -> Vec<Option<usize>> {
    let mut fences = crate::changelog::markdown::Fences::default();
    text.lines()
        .map(|line| if fences.is_code(line) { None } else { group_of(line) })
        .collect()
}

/// The changelog entries of a section: each `- ` bullet with its indented continuation lines, and the index of the
/// bump group it sits under (`None` in a flat section).
fn entries(section: &str) -> Vec<(Option<usize>, String)> {
    let mut entries: Vec<(Option<usize>, String)> = Vec::new();
    let mut group = None;
    let mut fences = crate::changelog::markdown::Fences::default();
    for line in section.lines() {
        let in_code = fences.is_code(line);
        if let Some(index) = group_of(line).filter(|_| !in_code) {
            group = Some(index);
            continue;
        }
        match entries.last_mut() {
            Some((_, last)) if line.starts_with(' ') || line.is_empty() => {
                last.push('\n');
                last.push_str(line);
            }
            _ if !in_code && line.starts_with("- ") => entries.push((group, line.to_owned())),
            _ => {}
        }
    }
    entries
        .into_iter()
        .map(|(group, entry)| (group, entry.trim_end().to_owned()))
        .collect()
}

/// `kept` (a grouped section) with each extra entry added to the bump group it came from. A group the section lacks is
/// created in order, an entry from a flat mate goes under Patch, and a group left with no entry is dropped.
fn merge_into_groups(kept: &str, extra: &[(Option<usize>, String)]) -> String {
    let mut preamble: Vec<&str> = Vec::new();
    let mut groups: [Vec<String>; 3] = Default::default();
    let mut current: Option<usize> = None;
    for (line, opens) in kept.lines().zip(group_openers(kept)) {
        match opens {
            Some(index) => current = Some(index),
            None => match current {
                Some(index) => groups[index].push(line.to_owned()),
                None => preamble.push(line),
            },
        }
    }
    for (group, entry) in extra {
        let body = &mut groups[group.unwrap_or(2)];
        while body.last().is_some_and(String::is_empty) {
            body.pop();
        }
        if body
            .last()
            .is_some_and(|last| !last.starts_with("- ") && !last.starts_with(' '))
        {
            body.push(String::new());
        }
        body.extend(entry.lines().map(str::to_owned));
    }
    let mut blocks: Vec<String> = Vec::new();
    let preamble = preamble.join("\n").trim().to_owned();
    if !preamble.is_empty() {
        blocks.push(preamble);
    }
    for (heading, body) in GROUP_HEADINGS.iter().zip(&groups) {
        let body = body.join("\n").trim().to_owned();
        if !body.is_empty() {
            blocks.push(format!("{heading}\n\n{body}"));
        }
    }
    blocks.join("\n\n")
}

/// The section with its group boilerplate bullets removed; every other line is kept as written.
fn without_boilerplate(section: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut skipping = false;
    for line in section.lines() {
        if is_group_boilerplate(line) {
            skipping = true;
        } else if skipping && line.starts_with(' ') {
            continue;
        } else {
            skipping = false;
            if !(line.is_empty() && kept.last().is_none_or(|last| last.is_empty())) {
                kept.push(line);
            }
        }
    }
    kept.join("\n").trim().to_owned()
}

/// Release notes for a package in a fixed group: its own changelog section, then the entries that the group's other
/// members recorded for the same version, each marked with the member it came from. The group is released as one
/// unit, so a change recorded against a member that owns no release still belongs in the release's notes. An entry
/// that is only the group boilerplate, or that the notes already hold, is left out. With no entry from another member
/// the notes are exactly [`release_notes`], and a package with no section of its own gets generated notes.
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
    let ReleaseNotes::Section(text) = &own else {
        return Ok(own);
    };
    let mut seen: HashSet<String> = entries(text)
        .iter()
        .filter_map(|(_, entry)| entry.strip_prefix("- "))
        .map(str::to_owned)
        .collect();
    let mut extra: Vec<(Option<usize>, String)> = Vec::new();
    for (name, member_changelog) in members {
        let Ok(section) = changelog_section(root, member_changelog, version)? else {
            continue;
        };
        for (group, entry) in entries(&section) {
            let Some(body) = entry.strip_prefix("- ") else {
                continue;
            };
            if is_group_boilerplate(&entry) || !seen.insert(body.to_owned()) {
                continue;
            }
            extra.push((group, format!("- `{name}`: {body}")));
        }
    }
    if extra.is_empty() {
        return Ok(own);
    }
    let kept = without_boilerplate(text);
    if group_openers(&kept).iter().any(Option::is_some) {
        return Ok(ReleaseNotes::Section(merge_into_groups(&kept, &extra)));
    }
    let extra = extra.into_iter().map(|(_, entry)| entry).collect::<Vec<_>>().join("\n");
    let ends_in_list = kept
        .lines()
        .last()
        .is_none_or(|last| last.starts_with("- ") || last.starts_with(' '));
    Ok(ReleaseNotes::Section(match (kept.is_empty(), ends_in_list) {
        (true, _) => extra,
        (false, true) => format!("{kept}\n{extra}"),
        (false, false) => format!("{kept}\n\n{extra}"),
    }))
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

    /// An owner with no changelog section gets generated notes, not its mates' entries.
    #[test]
    fn an_owner_without_a_section_gets_generated_notes() {
        let mate = Some("## 1.0.0\n\n- B gets a feature.\n");
        assert_eq!(
            group_notes(&[("cargo/a", Some("## 0.9.0\n\n- old\n")), ("cargo/b", mate)]),
            generated(NotesFallback::SectionMissing)
        );
        assert_eq!(
            group_notes(&[("cargo/a", None), ("cargo/b", mate)]),
            generated(NotesFallback::FileMissing)
        );
    }

    #[test]
    fn the_owners_headings_and_paragraphs_survive_a_mates_entries() {
        let owner = "## 1.0.0\n\nIntro paragraph.\n\n### Fixes\n\n- A fix.\n- Released together with the `lockstep` \
                     fixed group.\n\nClosing words.\n";
        let notes = group_notes(&[
            ("cargo/a", Some(owner)),
            ("cargo/b", Some("## 1.0.0\n\n- B gets a feature.\n")),
        ]);
        assert_eq!(
            notes,
            ReleaseNotes::Section(
                "Intro paragraph.\n\n### Fixes\n\n- A fix.\n\nClosing words.\n\n- `cargo/b`: B gets a feature."
                    .to_owned()
            )
        );
    }

    /// An entry that only appears inside a longer entry, or a longer one, is still its own entry.
    #[test]
    fn only_an_identical_entry_counts_as_already_held() {
        let notes = group_notes(&[
            ("cargo/a", Some("## 1.0.0\n\n- Fix the parser crash.\n")),
            ("cargo/b", Some("## 1.0.0\n\n- Fix\n- Fix the parser crash.\n")),
            ("cargo/c", Some("## 1.0.0\n\n- the parser\n- Fix\n")),
        ]);
        assert_eq!(
            notes,
            ReleaseNotes::Section("- Fix the parser crash.\n- `cargo/b`: Fix\n- `cargo/c`: the parser".to_owned())
        );
    }

    const GROUPED_OWNER: &str =
        "## 1.0.0\n\n_2026-11-08_\n\n### Minor Changes\n\n- Owner feature.\n\n### Patch Changes\n\n- Owner fix.\n";

    fn section(notes: ReleaseNotes) -> String {
        match notes {
            ReleaseNotes::Section(text) => text,
            other => panic!("expected a section, got {other:?}"),
        }
    }

    /// A mate's entry keeps the bump group it was recorded under: appending it to the owner's last group would file a
    /// minor change under Patch.
    #[test]
    fn a_mates_minor_entry_joins_the_owners_minor_group_not_the_last_group() {
        let mate = Some("## 1.0.0\n\n### Minor Changes\n\n- Mate feature.\n");
        let text = section(group_notes(&[("owner", Some(GROUPED_OWNER)), ("mate", mate)]));
        assert_eq!(
            text,
            "_2026-11-08_\n\n### Minor Changes\n\n- Owner feature.\n- `mate`: Mate feature.\n\n### Patch Changes\n\n- Owner fix."
        );
    }

    #[test]
    fn a_group_the_owner_lacks_is_created_in_order() {
        let owner = Some("## 1.0.0\n\n### Patch Changes\n\n- Owner fix.\n");
        let mate = Some("## 1.0.0\n\n### Major Changes\n\n- Big change.\n");
        let text = section(group_notes(&[("owner", owner), ("mate", mate)]));
        assert_eq!(
            text,
            "### Major Changes\n\n- `mate`: Big change.\n\n### Patch Changes\n\n- Owner fix."
        );
    }

    /// The owner's Patch group held only the group boilerplate; with the boilerplate gone and nothing added to it, the
    /// heading goes too.
    #[test]
    fn a_group_left_empty_by_the_boilerplate_is_dropped() {
        let owner = Some(
            "## 1.0.0\n\n### Minor Changes\n\n- Owner feature.\n\n### Patch Changes\n\n- Released together with the `lockstep` fixed group.\n",
        );
        let mate = Some("## 1.0.0\n\n### Minor Changes\n\n- Mate feature.\n");
        let text = section(group_notes(&[("owner", owner), ("mate", mate)]));
        assert_eq!(text, "### Minor Changes\n\n- Owner feature.\n- `mate`: Mate feature.");
    }

    #[test]
    fn an_entry_from_a_flat_mate_goes_under_patch() {
        let mate = Some("## 1.0.0\n\n- Flat mate entry.\n");
        let text = section(group_notes(&[("owner", Some(GROUPED_OWNER)), ("mate", mate)]));
        assert_eq!(
            text,
            "_2026-11-08_\n\n### Minor Changes\n\n- Owner feature.\n\n### Patch Changes\n\n- Owner fix.\n- `mate`: Flat mate entry."
        );
    }

    #[test]
    fn a_mate_entry_with_continuation_lines_keeps_them_inside_its_group() {
        let mate = Some("## 1.0.0\n\n### Minor Changes\n\n- Mate feature.\n\n  More detail.\n");
        let text = section(group_notes(&[("owner", Some(GROUPED_OWNER)), ("mate", mate)]));
        assert!(
            text.contains(
                "### Minor Changes\n\n- Owner feature.\n- `mate`: Mate feature.\n\n  More detail.\n\n### Patch Changes"
            ),
            "{text}"
        );
    }

    /// A group heading shown inside a fenced example is text, not a boundary: treating it as one would split the fence.
    #[test]
    fn a_group_heading_inside_a_fence_is_not_a_boundary() {
        let owner = Some(
            "## 1.0.0\n\n_2026-11-08_\n\n```md\n### Major Changes\n- example\n```\n\n### Minor Changes\n\n- Owner feature.\n",
        );
        let mate = Some("## 1.0.0\n\n### Minor Changes\n\n- Mate feature.\n");
        let text = section(group_notes(&[("owner", owner), ("mate", mate)]));
        assert_eq!(
            text,
            "_2026-11-08_\n\n```md\n### Major Changes\n- example\n```\n\n### Minor Changes\n\n- Owner feature.\n- `mate`: Mate feature."
        );
    }

    #[test]
    fn a_fence_inside_a_group_stays_whole_and_its_bullets_are_not_entries() {
        let owner = Some(
            "## 1.0.0\n\n### Minor Changes\n\n- Owner feature.\n\n```md\n### Patch Changes\n- not an entry\n```\n",
        );
        let mate = Some("## 1.0.0\n\n### Minor Changes\n\n- not an entry\n");
        let text = section(group_notes(&[("owner", owner), ("mate", mate)]));
        // The mate's entry shares text with the example bullet; only real entries make "seen", so it is added.
        assert!(text.contains("```md\n### Patch Changes\n- not an entry\n```"), "{text}");
        assert!(text.contains("- `mate`: not an entry"), "{text}");
        assert!(!text.contains("### Patch Changes\n\n- `mate`"), "{text}");
    }
}
