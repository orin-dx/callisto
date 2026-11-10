use callisto_model::{GroupKind, PackageId, Severity, Version};

use crate::changelog::{ChangeSource, ChangelogError, ChangelogInput, ChangelogLayout};

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

/// The bullet for one entry; `None` for a dependency update (rendered as one block) and for an empty summary.
fn entry_bullet(source: &ChangeSource) -> Option<String> {
    match source {
        ChangeSource::Changeset { summary, .. } => {
            indent_changeset_summary(summary).map(|indented| format!("- {indented}\n"))
        }
        ChangeSource::Commit { subject, sha } => Some(format!("- {subject} ({})\n", sha.short())),
        ChangeSource::DependencyUpdate { .. } => None,
        ChangeSource::PeerEscalation { dependency, to } => Some(format!(
            "- Peer dependency `{}` requires `{}`\n",
            dependency.display_name(),
            to.render()
        )),
        ChangeSource::GroupUnion { group, kind } => {
            let k_str = match kind {
                GroupKind::Fixed => "fixed",
                GroupKind::Linked => "linked",
            };
            Some(format!("- Released together with the `{group}` {k_str} group.\n"))
        }
        ChangeSource::NewGroupMember { group } => Some(format!("- Joined the `{group}` group at this version.\n")),
    }
}

fn dependency_block(dep_updates: &[(&PackageId, &Version)]) -> String {
    if dep_updates.is_empty() {
        return String::new();
    }
    let mut out = String::from("- Dependency updates\n");
    for (dep, to_ver) in dep_updates {
        out.push_str(&format!("  - `{}` → `{}`\n", dep.display_name(), to_ver.render()));
    }
    out
}

fn meta_line(date: Option<&str>, compare_url: Option<&str>) -> Option<String> {
    let link = compare_url.map(|url| format!("[compare]({url})"));
    match (date, link) {
        (Some(date), Some(link)) => Some(format!("_{date} · {link}_")),
        (Some(date), None) => Some(format!("_{date}_")),
        (None, Some(link)) => Some(format!("_{link}_")),
        (None, None) => None,
    }
}

pub fn render_section(input: &ChangelogInput) -> Result<String, ChangelogError> {
    if input.entries.is_empty() {
        return Err(ChangelogError::EmptyInput);
    }
    for entry in &input.entries {
        if entry.severity == Severity::None {
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

    let dep_updates: Vec<(&PackageId, &Version)> = input
        .entries
        .iter()
        .filter_map(|entry| match &entry.source {
            ChangeSource::DependencyUpdate { dependency, to, .. } => Some((dependency, to)),
            _ => None,
        })
        .collect();

    match input.style.layout {
        ChangelogLayout::Flat => {
            for entry in &input.entries {
                if let Some(bullet) = entry_bullet(&entry.source) {
                    out.push_str(&bullet);
                }
            }
            out.push_str(&dependency_block(&dep_updates));
        }
        ChangelogLayout::Grouped => {
            if let Some(line) = meta_line(input.style.date.as_deref(), input.style.compare_url.as_deref()) {
                out.push_str(&line);
                out.push_str("\n\n");
            }
            // A member with nothing of its own gets no bump heading: it did not make a minor or patch change.
            if input.entries.iter().all(|entry| {
                matches!(
                    entry.source,
                    ChangeSource::GroupUnion { .. } | ChangeSource::NewGroupMember { .. }
                )
            }) {
                out.extend(input.entries.iter().filter_map(|entry| entry_bullet(&entry.source)));
                return Ok(out);
            }
            let mut groups = Vec::new();
            for (severity, title) in [
                (Severity::Major, "Major Changes"),
                (Severity::Minor, "Minor Changes"),
                (Severity::Patch, "Patch Changes"),
            ] {
                let mut body: String = input
                    .entries
                    .iter()
                    .filter(|entry| entry.severity == severity)
                    .filter_map(|entry| entry_bullet(&entry.source))
                    .collect();
                // Dependency updates always sit under Patch, whatever severity their entry carries.
                if severity == Severity::Patch {
                    body.push_str(&dependency_block(&dep_updates));
                }
                if !body.is_empty() {
                    groups.push(format!("### {title}\n\n{body}"));
                }
            }
            out.push_str(&groups.join("\n"));
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
            style: Default::default(),
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
            style: Default::default(),
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
        // Entries with different authored severities must not show that distinction; the version heading carries the applied bump.
        let input = ChangelogInput {
            package: PackageId::parse("my-pkg").unwrap(),
            style: Default::default(),
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
            style: Default::default(),
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
            style: Default::default(),
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
            style: Default::default(),
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
            style: Default::default(),
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
                style: Default::default(),
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
            style: Default::default(),
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
            style: Default::default(),
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

    /// The renderer as it was before layouts existed, kept as the oracle for `flat`.
    fn legacy_render(input: &ChangelogInput) -> String {
        let mut out = format!("## {}\n\n", input.to.as_ref().unwrap().render());
        let mut dep_updates = Vec::new();
        for entry in &input.entries {
            match &entry.source {
                ChangeSource::Changeset { summary, .. } => {
                    if let Some(indented) = indent_changeset_summary(summary) {
                        out.push_str(&format!("- {indented}\n"));
                    }
                }
                ChangeSource::Commit { subject, sha } => out.push_str(&format!("- {subject} ({})\n", sha.short())),
                ChangeSource::DependencyUpdate { dependency, to, .. } => dep_updates.push((dependency, to)),
                ChangeSource::PeerEscalation { dependency, to } => out.push_str(&format!(
                    "- Peer dependency `{}` requires `{}`\n",
                    dependency.display_name(),
                    to.render()
                )),
                ChangeSource::GroupUnion { group, kind } => {
                    let k = if *kind == GroupKind::Fixed { "fixed" } else { "linked" };
                    out.push_str(&format!("- Released together with the `{group}` {k} group.\n"));
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
        out
    }

    fn entry(severity: Severity, source: ChangeSource) -> ChangelogEntry {
        ChangelogEntry { severity, source }
    }

    fn changeset(severity: Severity, summary: &str) -> ChangelogEntry {
        entry(
            severity,
            ChangeSource::Changeset {
                filename: "x.md".to_string(),
                summary: summary.to_string(),
            },
        )
    }

    fn dep_update(severity: Severity, dependency: &str, to: &str) -> ChangelogEntry {
        entry(
            severity,
            ChangeSource::DependencyUpdate {
                dependency: PackageId::parse(dependency).unwrap(),
                dep_kind: callisto_model::DepKind::Runtime,
                to: Version::parse(to, VersionGrammar::SemVer).unwrap(),
            },
        )
    }

    fn styled(
        entries: Vec<ChangelogEntry>,
        layout: ChangelogLayout,
        date: Option<&str>,
        url: Option<&str>,
    ) -> ChangelogInput {
        ChangelogInput {
            package: PackageId::parse("my-pkg").unwrap(),
            from: Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            to: Some(Version::parse("1.2.0", VersionGrammar::SemVer).unwrap()),
            entries,
            style: crate::changelog::ChangelogStyle {
                layout,
                date: date.map(str::to_owned),
                compare_url: url.map(str::to_owned),
            },
        }
    }

    fn mixed_entries() -> Vec<ChangelogEntry> {
        vec![
            changeset(Severity::Patch, "Fix one.\n\n- detail"),
            changeset(Severity::Major, "Break it."),
            entry(
                Severity::Minor,
                ChangeSource::Commit {
                    sha: callisto_model::CommitSha::parse(&"b".repeat(40)).unwrap(),
                    subject: "Add two".to_string(),
                },
            ),
            dep_update(Severity::Patch, "dep-a", "2.0.0"),
            entry(
                Severity::Minor,
                ChangeSource::PeerEscalation {
                    dependency: PackageId::parse("peer").unwrap(),
                    to: Version::parse("3.0.0", VersionGrammar::SemVer).unwrap(),
                },
            ),
            entry(
                Severity::Patch,
                ChangeSource::GroupUnion {
                    group: callisto_model::GroupName("g".to_string()),
                    kind: GroupKind::Linked,
                },
            ),
            entry(
                Severity::Patch,
                ChangeSource::NewGroupMember {
                    group: callisto_model::GroupName("g".to_string()),
                },
            ),
            dep_update(Severity::Patch, "dep-b", "1.5.0"),
            changeset(Severity::Patch, ""),
        ]
    }

    #[test]
    fn flat_layout_is_byte_identical_to_the_pre_layout_renderer() {
        let with_facts = styled(
            mixed_entries(),
            ChangelogLayout::Flat,
            Some("2026-11-08"),
            Some("https://github.com/o/r/compare/a...b"),
        );
        let plain = styled(mixed_entries(), ChangelogLayout::Flat, None, None);
        let expected = legacy_render(&plain);
        assert_eq!(render_section(&plain).unwrap(), expected);
        assert_eq!(
            render_section(&with_facts).unwrap(),
            expected,
            "flat ignores date and link"
        );
        assert!(!expected.contains("###"));
    }

    #[test]
    fn grouped_layout_orders_groups_and_keeps_entry_order_inside_each() {
        let input = styled(
            mixed_entries(),
            ChangelogLayout::Grouped,
            Some("2026-11-08"),
            Some("https://github.com/o/r/compare/a...b"),
        );
        let expected = "## 1.2.0\n\n\
            _2026-11-08 · [compare](https://github.com/o/r/compare/a...b)_\n\n\
            ### Major Changes\n\n- Break it.\n\n\
            ### Minor Changes\n\n- Add two (bbbbbbb)\n- Peer dependency `peer` requires `3.0.0`\n\n\
            ### Patch Changes\n\n- Fix one.\n\n  - detail\n\
            - Released together with the `g` linked group.\n\
            - Joined the `g` group at this version.\n\
            - Dependency updates\n  - `dep-a` → `2.0.0`\n  - `dep-b` → `1.5.0`\n";
        assert_eq!(render_section(&input).unwrap(), expected);
    }

    #[test]
    fn grouped_layout_gives_a_group_only_member_no_bump_heading() {
        let group = callisto_model::GroupName("g".to_string());
        let input = styled(
            vec![entry(
                Severity::Minor,
                ChangeSource::GroupUnion {
                    group: group.clone(),
                    kind: callisto_model::GroupKind::Fixed,
                },
            )],
            ChangelogLayout::Grouped,
            Some("2026-11-08"),
            None,
        );
        assert_eq!(
            render_section(&input).unwrap(),
            "## 1.2.0\n\n_2026-11-08_\n\n- Released together with the `g` fixed group.\n"
        );

        let joined = styled(
            vec![entry(Severity::Minor, ChangeSource::NewGroupMember { group })],
            ChangelogLayout::Grouped,
            None,
            None,
        );
        assert_eq!(
            render_section(&joined).unwrap(),
            "## 1.2.0\n\n- Joined the `g` group at this version.\n"
        );
    }

    #[test]
    fn grouped_layout_omits_empty_groups() {
        let input = styled(
            vec![changeset(Severity::Minor, "Only minor.")],
            ChangelogLayout::Grouped,
            None,
            None,
        );
        assert_eq!(
            render_section(&input).unwrap(),
            "## 1.2.0\n\n### Minor Changes\n\n- Only minor.\n"
        );
    }

    /// A group whose only entries render nothing (empty summaries) must not leave a bare heading.
    #[test]
    fn grouped_layout_omits_a_group_whose_entries_are_all_empty() {
        let input = styled(
            vec![changeset(Severity::Major, ""), changeset(Severity::Patch, "Real.")],
            ChangelogLayout::Grouped,
            None,
            None,
        );
        let rendered = render_section(&input).unwrap();
        assert!(!rendered.contains("Major Changes"), "{rendered}");
        assert!(rendered.contains("### Patch Changes\n\n- Real.\n"), "{rendered}");
    }

    /// Dependency updates sit under Patch Changes even when their entry carries a higher severity.
    #[test]
    fn grouped_layout_puts_dependency_updates_under_patch() {
        let input = styled(
            vec![
                changeset(Severity::Minor, "Feature."),
                dep_update(Severity::Minor, "dep-a", "2.0.0"),
            ],
            ChangelogLayout::Grouped,
            None,
            None,
        );
        assert_eq!(
            render_section(&input).unwrap(),
            "## 1.2.0\n\n### Minor Changes\n\n- Feature.\n\n### Patch Changes\n\n- Dependency updates\n  - `dep-a` → `2.0.0`\n"
        );
    }

    #[test]
    fn grouped_header_line_shows_whichever_facts_are_known() {
        let line = |date: Option<&str>, url: Option<&str>| {
            let input = styled(
                vec![changeset(Severity::Patch, "x")],
                ChangelogLayout::Grouped,
                date,
                url,
            );
            render_section(&input).unwrap().lines().nth(2).unwrap().to_owned()
        };
        assert_eq!(line(Some("2026-11-08"), None), "_2026-11-08_");
        assert_eq!(
            line(Some("2026-11-08"), Some("https://x/compare/a...b")),
            "_2026-11-08 · [compare](https://x/compare/a...b)_"
        );
        assert_eq!(
            line(None, Some("https://x/compare/a...b")),
            "_[compare](https://x/compare/a...b)_"
        );
        assert_eq!(line(None, None), "### Patch Changes");
    }
}
