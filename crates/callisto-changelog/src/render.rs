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
        out.push('\n');
        out.push_str(section_name);
        out.push('\n');
        out.push('\n');

        let mut dep_updates = Vec::new();

        for entry in entries {
            match &entry.source {
                ChangeSource::Changeset { summary, .. } => {
                    out.push_str(&format!("- {summary}\n"));
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
}
