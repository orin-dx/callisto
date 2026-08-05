use callisto_model::GroupKind;

use crate::{group_entries, ChangeSource, ChangelogError, ChangelogInput};

pub fn render_section(input: &ChangelogInput) -> Result<String, ChangelogError> {
    if input.entries.is_empty() {
        return Err(ChangelogError::EmptyInput);
    }
    let grouped = group_entries(&input.entries)?;
    let mut out = String::new();

    let heading = match &input.to {
        Some(v) => format!("## {}", v.render()),
        None => "## Unreleased".to_string(),
    };
    out.push_str(&heading);
    out.push('\n');

    for (section_name, entries) in [
        ("### Major Changes", &grouped.major),
        ("### Minor Changes", &grouped.minor),
        ("### Patch Changes", &grouped.patch),
    ] {
        if entries.is_empty() {
            continue;
        }

        // Pre-filter: only emit the section heading when at least one entry
        // will produce visible output. Changeset entries with blank summaries
        // are silently skipped in the loop below, so a section consisting
        // entirely of such entries must not emit its heading.
        let has_visible_output = entries.iter().any(|e| match &e.source {
            ChangeSource::Changeset { summary, .. } => !summary.trim().is_empty(),
            _ => true,
        });
        if !has_visible_output {
            continue;
        }

        out.push('\n');
        out.push_str(section_name);
        out.push('\n');
        out.push('\n');

        let mut dep_updates = Vec::new();

        for entry in entries {
            match &entry.source {
                ChangeSource::Changeset { summary, .. } => {
                    if summary.trim().is_empty() {
                        continue;
                    }
                    let indented = summary.trim_end_matches('\n');
                    let indented = indented.replace('\n', "\n  ");
                    out.push_str(&format!("- {indented}\n"));
                }
                ChangeSource::Commit { subject, sha } => {
                    out.push_str(&format!("- {subject} ({})\n", sha.short()));
                }
                ChangeSource::DependencyUpdate { dependency, to, .. } => {
                    dep_updates.push((dependency, to));
                }
                ChangeSource::PeerEscalation { dependency, to } => {
                    out.push_str(&format!(
                        "- Peer dependency `{}` requires `{}`\n",
                        dependency.display_name(),
                        to.render()
                    ));
                }
                ChangeSource::GroupUnion { group, kind } => {
                    let k_str = match kind {
                        GroupKind::Fixed => "fixed",
                        GroupKind::Linked => "linked",
                    };
                    out.push_str(&format!(
                        "- Released together with the `{group}` {k_str} group.\n"
                    ));
                }
                ChangeSource::NewGroupMember { group } => {
                    out.push_str(&format!("- Joined the `{group}` group at this version.\n"));
                }
            }
        }

        if !dep_updates.is_empty() {
            out.push_str("- Dependency updates\n");
            for (dep, to_ver) in dep_updates {
                out.push_str(&format!(
                    "  - `{}` → `{}`\n",
                    dep.display_name(),
                    to_ver.render()
                ));
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChangelogEntry;
    use callisto_model::{PackageId, Severity, Version, VersionGrammar};

    fn make_input(summary: &str) -> ChangelogInput {
        ChangelogInput {
            package: PackageId::parse("my-pkg").unwrap(),
            from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            to: Some(Version::parse("1.1.0", VersionGrammar::SemVer).unwrap()),
            entries: vec![ChangelogEntry {
                severity: Severity::Patch,
                source: ChangeSource::Changeset {
                    filename: "fix.md".to_string(),
                    summary: summary.to_string(),
                },
            }],
        }
    }

    #[test]
    fn renders_basic_changelog_section() {
        let input = ChangelogInput {
            package: PackageId::parse("my-pkg").unwrap(),
            from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            to: Some(Version::parse("1.1.0", VersionGrammar::SemVer).unwrap()),
            entries: vec![ChangelogEntry {
                severity: Severity::Minor,
                source: ChangeSource::Changeset {
                    filename: "cool-dragons.md".to_string(),
                    summary: "Add dragon feature".to_string(),
                },
            }],
        };

        let rendered = render_section(&input).unwrap();
        assert!(rendered.contains("## 1.1.0"));
        assert!(rendered.contains("### Minor Changes"));
        assert!(rendered.contains("- Add dragon feature"));
    }

    #[test]
    fn render_section_indents_multiline_summary_continuation_lines() {
        let input = make_input("First line.\n\nSecond paragraph.");
        let rendered = render_section(&input).unwrap();
        // Continuation lines must be indented — bare "\nSecond paragraph." is wrong
        assert!(
            !rendered.contains("\nSecond paragraph."),
            "continuation line must not appear unindented; got:\n{rendered}"
        );
        // Must appear with at least 2-space indent
        assert!(
            rendered.contains("\n  Second paragraph."),
            "continuation line must be indented by 2 spaces; got:\n{rendered}"
        );
    }

    #[test]
    fn render_section_handles_trailing_newline_in_summary() {
        let input = make_input("Fix bug.\n");
        let rendered = render_section(&input).unwrap();
        // A trailing newline in the summary must not produce a doubled blank line
        assert!(
            !rendered.contains("- Fix bug.\n\n"),
            "trailing newline in summary must not produce a doubled blank line; got:\n{rendered}"
        );
    }

    #[test]
    fn test_render_section_all_empty_summaries_no_heading() {
        // CORR-002: when every entry in a section has a blank summary the
        // section heading must not be emitted at all.
        let input = ChangelogInput {
            package: PackageId::parse("my-pkg").unwrap(),
            from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            to: Some(Version::parse("1.1.0", VersionGrammar::SemVer).unwrap()),
            entries: vec![ChangelogEntry {
                severity: Severity::Patch,
                source: ChangeSource::Changeset {
                    filename: "empty.md".to_string(),
                    summary: "".to_string(),
                },
            }],
        };

        let rendered = render_section(&input).unwrap();
        assert!(
            !rendered.contains("### Patch Changes"),
            "section heading must not be emitted when all summaries are empty; got:\n{rendered}"
        );
    }

    #[test]
    fn render_section_skips_entry_with_empty_summary() {
        let input = ChangelogInput {
            package: PackageId::parse("my-pkg").unwrap(),
            from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            to: Some(Version::parse("1.1.0", VersionGrammar::SemVer).unwrap()),
            entries: vec![
                ChangelogEntry {
                    severity: Severity::Patch,
                    source: ChangeSource::Changeset {
                        filename: "empty.md".to_string(),
                        summary: "".to_string(),
                    },
                },
                ChangelogEntry {
                    severity: Severity::Patch,
                    source: ChangeSource::Changeset {
                        filename: "real.md".to_string(),
                        summary: "Real change".to_string(),
                    },
                },
            ],
        };

        let rendered = render_section(&input).unwrap();
        assert!(
            !rendered.contains("- \n"),
            "empty summary must not produce a blank bullet; got:\n{rendered}"
        );
        assert!(
            rendered.contains("- Real change"),
            "non-empty entry must still appear; got:\n{rendered}"
        );
    }
}
