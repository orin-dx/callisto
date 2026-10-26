use callisto_model::GroupKind;

use crate::changelog::{ChangeSource, ChangelogError, ChangelogInput};

pub(crate) fn indent_changeset_summary(summary: &str) -> Option<String> {
    let summary = summary.trim_end_matches('\n');
    if summary.trim().is_empty() {
        return None;
    }
    let mut indented = String::new();
    for (index, line) in summary.lines().enumerate() {
        if index > 0 {
            indented.push('\n');
        }
        if line.trim().is_empty() {
            continue;
        }
        if index > 0 {
            indented.push_str("  ");
        }
        indented.push_str(line);
    }
    Some(indented)
}

pub fn render_section(input: &ChangelogInput) -> Result<String, ChangelogError> {
    if input.entries.is_empty() {
        return Err(ChangelogError::EmptyInput);
    }
    for entry in &input.entries {
        if entry.severity == callisto_model::Severity::None {
            return Err(ChangelogError::SeverityNoneEntry);
        }
    }

    let mut out = String::new();

    let heading = match &input.to {
        Some(v) => format!("## {}", v.render()),
        None => "## Unreleased".to_string(),
    };
    out.push_str(&heading);
    out.push('\n');
    out.push('\n');

    let mut dep_updates = Vec::new();

    for entry in &input.entries {
        match &entry.source {
            ChangeSource::Changeset { summary, .. } => {
                if let Some(indented) = indent_changeset_summary(summary) {
                    out.push_str(&format!("- {indented}\n"));
                }
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
                out.push_str(&format!("- Released together with the `{group}` {k_str} group.\n"));
            }
            ChangeSource::NewGroupMember { group } => {
                out.push_str(&format!("- Joined the `{group}` group at this version.\n"));
            }
        }
    }

    if !dep_updates.is_empty() {
        out.push_str("- Dependency updates\n");
        for (dep, to_ver) in dep_updates {
            out.push_str(&format!("  - `{}` → `{}`\n", dep.display_name(), to_ver.render()));
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changelog::ChangelogEntry;
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
        assert!(!rendered.contains("### Minor Changes"));
        assert!(!rendered.contains("### Patch Changes"));
        assert!(rendered.contains("- Add dragon feature"));
    }

    #[test]
    fn render_section_flattens_mixed_severities_into_one_list() {
        // A package released alongside a fixed group can carry entries of
        // different originally-authored severities (e.g. a patch-level fix
        // plus a group-driven minor bump). The rendered changelog must not
        // re-surface that per-entry severity distinction -- the version
        // heading already communicates the one, real applied bump.
        let input = ChangelogInput {
            package: PackageId::parse("my-pkg").unwrap(),
            from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            to: Some(Version::parse("1.1.0", VersionGrammar::SemVer).unwrap()),
            entries: vec![
                ChangelogEntry {
                    severity: Severity::Minor,
                    source: ChangeSource::GroupUnion {
                        group: callisto_model::GroupName("workspace".to_string()),
                        kind: GroupKind::Fixed,
                    },
                },
                ChangelogEntry {
                    severity: Severity::Patch,
                    source: ChangeSource::Changeset {
                        filename: "fix.md".to_string(),
                        summary: "Fix a bug".to_string(),
                    },
                },
            ],
        };

        let rendered = render_section(&input).unwrap();
        assert!(!rendered.contains("### Major Changes"));
        assert!(!rendered.contains("### Minor Changes"));
        assert!(!rendered.contains("### Patch Changes"));
        assert!(rendered.contains("- Released together with the `workspace` fixed group."));
        assert!(rendered.contains("- Fix a bug"));
    }

    #[test]
    fn render_section_indents_multiline_summary_continuation_lines() {
        let input = make_input("First line.\n\n- A detail.");
        let rendered = render_section(&input).unwrap();
        assert_eq!(rendered, "## 1.1.0\n\n- First line.\n\n  - A detail.\n");
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
    fn render_section_severity_none_entry_is_rejected() {
        let input = ChangelogInput {
            package: PackageId::parse("my-pkg").unwrap(),
            from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            to: Some(Version::parse("1.1.0", VersionGrammar::SemVer).unwrap()),
            entries: vec![ChangelogEntry {
                severity: Severity::None,
                source: ChangeSource::Changeset {
                    filename: "bad.md".to_string(),
                    summary: "Should never happen".to_string(),
                },
            }],
        };

        assert!(matches!(render_section(&input), Err(ChangelogError::SeverityNoneEntry)));
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

    #[test]
    fn render_section_renders_commit_source() {
        let input = ChangelogInput {
            package: PackageId::parse("my-pkg").unwrap(),
            from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            to: Some(Version::parse("1.1.0", VersionGrammar::SemVer).unwrap()),
            entries: vec![ChangelogEntry {
                severity: Severity::Patch,
                source: ChangeSource::Commit {
                    sha: callisto_model::CommitSha::parse(&"a".repeat(40)).unwrap(),
                    subject: "Fix the thing".to_string(),
                },
            }],
        };

        let rendered = render_section(&input).unwrap();
        assert!(
            rendered.contains("- Fix the thing ("),
            "expected commit subject and short sha, got:\n{rendered}"
        );
    }

    #[test]
    fn render_section_renders_peer_escalation_source() {
        let input = ChangelogInput {
            package: PackageId::parse("my-pkg").unwrap(),
            from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            to: Some(Version::parse("1.1.0", VersionGrammar::SemVer).unwrap()),
            entries: vec![ChangelogEntry {
                severity: Severity::Patch,
                source: ChangeSource::PeerEscalation {
                    dependency: PackageId::parse("peer-pkg").unwrap(),
                    to: Version::parse("2.0.0", VersionGrammar::SemVer).unwrap(),
                },
            }],
        };

        let rendered = render_section(&input).unwrap();
        assert!(
            rendered.contains("Peer dependency `peer-pkg` requires `2.0.0`"),
            "got:\n{rendered}"
        );
    }

    #[test]
    fn render_section_renders_group_union_source_for_both_kinds() {
        for (kind, expected_word) in [
            (callisto_model::GroupKind::Fixed, "fixed"),
            (callisto_model::GroupKind::Linked, "linked"),
        ] {
            let input = ChangelogInput {
                package: PackageId::parse("my-pkg").unwrap(),
                from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
                to: Some(Version::parse("1.1.0", VersionGrammar::SemVer).unwrap()),
                entries: vec![ChangelogEntry {
                    severity: Severity::Patch,
                    source: ChangeSource::GroupUnion {
                        group: callisto_model::GroupName("workspace".to_string()),
                        kind,
                    },
                }],
            };

            let rendered = render_section(&input).unwrap();
            assert!(
                rendered.contains(&format!(
                    "Released together with the `workspace` {expected_word} group."
                )),
                "kind={kind:?}, got:\n{rendered}"
            );
        }
    }

    #[test]
    fn render_section_renders_new_group_member_source() {
        let input = ChangelogInput {
            package: PackageId::parse("my-pkg").unwrap(),
            from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            to: Some(Version::parse("1.1.0", VersionGrammar::SemVer).unwrap()),
            entries: vec![ChangelogEntry {
                severity: Severity::Patch,
                source: ChangeSource::NewGroupMember {
                    group: callisto_model::GroupName("workspace".to_string()),
                },
            }],
        };

        let rendered = render_section(&input).unwrap();
        assert!(
            rendered.contains("Joined the `workspace` group at this version."),
            "got:\n{rendered}"
        );
    }

    #[test]
    fn render_section_to_none_renders_unreleased_heading() {
        let input = ChangelogInput {
            package: PackageId::parse("my-pkg").unwrap(),
            from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            to: None,
            entries: vec![ChangelogEntry {
                severity: Severity::Patch,
                source: ChangeSource::Changeset {
                    filename: "fix.md".to_string(),
                    summary: "A change".to_string(),
                },
            }],
        };

        let rendered = render_section(&input).unwrap();
        assert!(
            rendered.starts_with("## Unreleased\n\n"),
            "to: None must render an \"## Unreleased\" heading, got:\n{rendered}"
        );
    }
}
