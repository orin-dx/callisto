use std::collections::HashMap;

use indexmap::IndexMap;

use callisto_changelog::ChangeSource;
use callisto_model::{CommandRunner, ComposePrBodyReport, PackageId, Severity, SCHEMA_VERSION};

use crate::commands::version::plan_version;
use crate::error::GraphError;
use crate::infer::SeverityInference;
use crate::resolver::DependencyResolver;
use crate::Workspace;

#[derive(Clone, Debug, Default)]
pub struct PrBodyOptions {
    pub existing_body: Option<String>,
    pub branch: Option<String>,
}

fn severity_emoji(severity: Severity) -> &'static str {
    match severity {
        Severity::Major => "🔴",
        Severity::Minor => "🟡",
        Severity::Patch => "🟢",
        Severity::None => "⚪",
    }
}

/// Identity used to merge the same underlying change across every package it
/// bumps, instead of repeating its rendered text once per package. A shared
/// `.changeset/*.md` file's `summary` is copied verbatim into every package
/// it names (§6.1's fan-out model) -- correct and desirable for each
/// package's own standalone `CHANGELOG.md`, but exactly the duplication that
/// makes a multi-package PR body hard to read in one sitting.
#[derive(Clone, PartialEq, Eq, Hash)]
enum ChangeGroupKey {
    Changeset(String),
    Commit(String),
    PeerEscalation(PackageId, String),
    DependencyUpdate(PackageId, String),
    /// Never merged across packages: one `ChangelogWrite` per package whose
    /// `render_section` failed (e.g. an empty or `Severity::None` entry) or
    /// was entirely absent, falling back to `bump.reason`-derived text --
    /// package-specific by construction, so each gets its own slot.
    Fallback(usize),
}

struct ChangeGroup {
    /// Rendered once, from the first (and, for real changesets, only
    /// possible) occurrence -- every member's copy is byte-identical by
    /// construction for `Changeset`/`Commit` keys.
    body: String,
    /// `(package, this entry's own severity)` -- deliberately the entry's
    /// severity, not the package's overall post-cascade `bump.severity`,
    /// since one package can carry several entries at different severities.
    members: Vec<(PackageId, Severity)>,
    /// `Fallback` groups render like a standalone per-package block (their
    /// one member's name becomes the heading); real changes render as
    /// "<key> -- affects: pkg, pkg".
    is_fallback: bool,
}

impl ChangeGroup {
    fn has_major(&self) -> bool {
        self.members.iter().any(|(_, severity)| *severity == Severity::Major)
    }
}

/// Renders one changelog entry's bullet text in isolation, mirroring
/// `callisto_changelog::render_section`'s per-source formatting exactly
/// (minus its cross-entry `DependencyUpdate` batching, which does not apply
/// once entries are regrouped across packages by shared identity instead of
/// by owning package). Returns `None` for a `Changeset`/`Commit`-equivalent
/// source with nothing to say (matching `render_section`'s own
/// empty-summary skip).
fn render_entry_text(source: &ChangeSource) -> Option<String> {
    match source {
        ChangeSource::Changeset { summary, .. } => {
            if summary.trim().is_empty() {
                return None;
            }
            let indented = summary.trim_end_matches('\n');
            Some(indented.replace('\n', "\n  "))
        }
        ChangeSource::Commit { subject, sha } => Some(format!("{subject} ({})", sha.short())),
        ChangeSource::DependencyUpdate { dependency, to, .. } => {
            Some(format!("`{}` → `{}`", dependency.display_name(), to.render()))
        }
        ChangeSource::PeerEscalation { dependency, to } => Some(format!(
            "Peer dependency `{}` requires `{}`",
            dependency.display_name(),
            to.render()
        )),
        // GroupUnion/NewGroupMember carry no content of their own beyond
        // "this package moved because its group did" -- already visible in
        // the summary table's Reason column, so they are deliberately
        // excluded from `What's Changing` entirely rather than rendered as
        // a redundant one-line block (see `change_group_key`'s matching arm
        // and the bystander-note logic in `render_pr_body_from_plan`).
        ChangeSource::GroupUnion { .. } | ChangeSource::NewGroupMember { .. } => None,
    }
}

fn change_group_key(source: &ChangeSource) -> Option<ChangeGroupKey> {
    match source {
        ChangeSource::Changeset { filename, .. } => Some(ChangeGroupKey::Changeset(filename.clone())),
        ChangeSource::Commit { sha, .. } => Some(ChangeGroupKey::Commit(sha.as_str().to_string())),
        ChangeSource::PeerEscalation { dependency, to } => Some(ChangeGroupKey::PeerEscalation(
            dependency.clone(),
            to.render().to_string(),
        )),
        ChangeSource::DependencyUpdate { dependency, to, .. } => Some(ChangeGroupKey::DependencyUpdate(
            dependency.clone(),
            to.render().to_string(),
        )),
        ChangeSource::GroupUnion { .. } | ChangeSource::NewGroupMember { .. } => None,
    }
}

/// A short, stable label for a change group's `<summary>` -- derived from
/// its identity (a changeset filename, a commit, a dependency bump), never
/// from sniffing the free-text body, so it stays predictable even when a
/// changeset's summary doesn't start with a bolded headline.
fn change_group_label(key: &ChangeGroupKey) -> String {
    match key {
        ChangeGroupKey::Changeset(filename) => {
            format!("`{}`", filename.strip_suffix(".md").unwrap_or(filename))
        }
        ChangeGroupKey::Commit(sha) => format!("Commit `{}`", &sha[..sha.len().min(7)]),
        ChangeGroupKey::PeerEscalation(dep, to) => {
            format!("Peer dependency `{}` → `{}`", dep.display_name(), to)
        }
        ChangeGroupKey::DependencyUpdate(dep, to) => format!("Dependency `{}` → `{}`", dep.display_name(), to),
        ChangeGroupKey::Fallback(_) => unreachable!("Fallback groups render via their sole member, not this label"),
    }
}

/// Text for a package whose `ChangelogWrite` is absent or failed to render
/// (e.g. an empty or `Severity::None` entry) -- derived from `bump.reason`
/// instead. Package-specific by construction (a `BumpReason` describes one
/// package's own severity computation), so never merged across packages.
fn fallback_reason_text(bump: &crate::plan::PlannedBump) -> String {
    match &bump.reason {
        Some(reason) => match reason {
            callisto_model::BumpReason::Changeset { changesets } => {
                format!("- Associated changesets: `{}`\n", changesets.join("`, `"))
            }
            callisto_model::BumpReason::Cascade {
                via,
                spec,
                dependency_to,
                ..
            } => {
                format!(
                    "- Automatic dependency cascade triggered by `{}` (spec `{}` target `{}`).\n",
                    via.display_name(),
                    spec,
                    dependency_to.render()
                )
            }
            callisto_model::BumpReason::PeerEscalation { via, spec } => {
                format!(
                    "- Peer dependency escalation triggered by `{}` (spec `{}`).\n",
                    via.display_name(),
                    spec
                )
            }
            callisto_model::BumpReason::FixedGroupUnion { group } => {
                format!("- Fixed group synchronization for group `{}`.\n", group.as_str())
            }
            callisto_model::BumpReason::LinkedGroupUnion { group } => {
                format!("- Linked group version alignment for group `{}`.\n", group.as_str())
            }
            callisto_model::BumpReason::Inference { commits, .. } => {
                format!("- Inferred from {commits} commit(s) since the last release.\n")
            }
            callisto_model::BumpReason::PreRelease { tag } => {
                format!("- Pre-release finalization (tag `{tag}`).\n")
            }
            callisto_model::BumpReason::NewGroupMember { group } => {
                format!("- New member of group `{}`.\n", group.as_str())
            }
            _ => "_No changelog entries available._\n".to_string(),
        },
        None => "_No changelog entries available._\n".to_string(),
    }
}

pub fn compose_pr_body<R: CommandRunner, D: DependencyResolver, I: SeverityInference>(
    ws: &Workspace<'_, R, D>,
    inference: &I,
    opts: &PrBodyOptions,
) -> Result<ComposePrBodyReport, GraphError> {
    let plan = plan_version(ws, inference, &crate::commands::version::VersionOptions::default())?;
    render_pr_body_from_plan(&plan, opts)
}

pub fn render_pr_body_from_plan(
    plan: &crate::plan::VersionPlan,
    opts: &PrBodyOptions,
) -> Result<ComposePrBodyReport, GraphError> {
    let mut body = String::new();

    // Preserve custom user notes above ## Release Preview if an existing body was passed
    if let Some(ref existing) = opts.existing_body {
        if let Some((prefix, _)) = existing.split_once("## Release Preview") {
            if !prefix.trim().is_empty() {
                body.push_str(prefix);
                if !prefix.ends_with('\n') {
                    body.push('\n');
                }
            }
        }
    }

    body.push_str("## Release Preview\n\n");
    body.push_str("This automated PR was generated by [Callisto](https://github.com/orin-dx/callisto). Merging this PR will publish the updated packages to their release targets.\n\n");

    if plan.bumps.is_empty() {
        body.push_str("> [!NOTE]\n");
        body.push_str("> **No pending changesets found.** Workspace packages are currently up to date.\n\n");
        let branch = opts.branch.as_deref().unwrap_or("callisto/version-packages");
        body.push_str("<details>\n<summary><b>Release Workflow Instructions</b></summary>\n\n");
        body.push_str("- **Merging this PR**: Merging into `main` will automatically publish all updated packages to their release registries.\n");
        body.push_str("- **Adding more changesets**: If additional changesets are pushed to `main`, Callisto will automatically re-calculate and update this PR.\n");
        body.push_str(&format!(
            "- **Manual edits**: You can make manual adjustments directly on the `{}` branch if needed.\n\n",
            branch
        ));
        body.push_str("</details>\n");
        return Ok(ComposePrBodyReport {
            schema_version: SCHEMA_VERSION,
            body,
            diagnostics: Vec::new(),
        });
    }

    // 1. Executive Summary Table
    body.push_str("### 📊 Release Summary\n\n");
    body.push_str("| Package | Ecosystem | Current | Target | Bump | Reason |\n");
    body.push_str("| :--- | :--- | :--- | :--- | :--- | :--- |\n");

    let mut has_major = false;

    for bump in &plan.bumps {
        if bump.severity == callisto_model::Severity::Major {
            has_major = true;
        }

        let eco = bump.package.ecosystem().map(|e| e.prefix()).unwrap_or("core");

        let reason_str = match &bump.reason {
            Some(callisto_model::BumpReason::Changeset { changesets }) => {
                format!("Changeset (`{}`)", changesets.join("`, `"))
            }
            Some(callisto_model::BumpReason::Cascade { via, .. }) => {
                format!("Cascade from `{}`", via.display_name())
            }
            Some(callisto_model::BumpReason::PeerEscalation { via, .. }) => {
                format!("Peer escalation from `{}`", via.display_name())
            }
            Some(callisto_model::BumpReason::FixedGroupUnion { group }) => {
                format!("Fixed group `{}`", group.as_str())
            }
            Some(callisto_model::BumpReason::LinkedGroupUnion { group }) => {
                format!("Linked group `{}`", group.as_str())
            }
            Some(callisto_model::BumpReason::Inference { commits, .. }) => {
                format!("Inferred ({commits} commits)")
            }
            Some(callisto_model::BumpReason::PreRelease { tag }) => {
                format!("Prerelease (`{tag}`)")
            }
            Some(callisto_model::BumpReason::NewGroupMember { group }) => {
                format!("New member (`{}`)", group.as_str())
            }
            _ => "Package bump".to_string(),
        };

        body.push_str(&format!(
            "| `{}` | `{}` | `{}` | **`{}`** | {} `{}` | {} |\n",
            bump.package.display_name(),
            eco,
            bump.from.render(),
            bump.to.render(),
            severity_emoji(bump.severity),
            bump.severity,
            reason_str
        ));
    }

    body.push('\n');

    // 2. Alert Callouts
    if has_major {
        body.push_str("> [!WARNING]\n");
        body.push_str("> **Major Version Bumps Detected**: This release contains major breaking changes. Review the changes below before merging.\n\n");
    } else {
        body.push_str("> [!NOTE]\n");
        body.push_str(&format!(
            "> **{} package(s)** queued for versioning. All changes are backward compatible.\n\n",
            plan.bumps.len()
        ));
    }

    // 3. What's Changing -- grouped by underlying change, not by package, so
    // a changeset naming several packages (§6.1's fan-out) renders once
    // instead of once per package it touches.
    body.push_str("### 📦 What's Changing\n\n");

    // PERF: build a map once so each lookup inside the loop is O(1)
    // instead of O(N) (the previous changelog_writes().find() call).
    let changelog_write_map: HashMap<_, _> = plan.changelog_writes.iter().map(|w| (&w.input.package, w)).collect();

    let mut groups: IndexMap<ChangeGroupKey, ChangeGroup> = IndexMap::new();
    let mut bystanders: Vec<PackageId> = Vec::new();
    let mut fallback_counter = 0usize;

    for bump in &plan.bumps {
        let matching_write = changelog_write_map.get(&bump.package).copied();

        let section_ok = matching_write.is_some_and(|w| callisto_changelog::render_section(&w.input).is_ok());

        if section_ok {
            let entries = &matching_write
                .expect("section_ok implies a matching write")
                .input
                .entries;
            let mut has_substantive_entry = false;

            for entry in entries {
                let Some(key) = change_group_key(&entry.source) else {
                    continue; // GroupUnion/NewGroupMember: no content of its own, see render_entry_text
                };
                let Some(text) = render_entry_text(&entry.source) else {
                    continue; // empty summary, matches render_section's own skip
                };
                has_substantive_entry = true;
                groups
                    .entry(key)
                    .or_insert_with(|| ChangeGroup {
                        body: text,
                        members: Vec::new(),
                        is_fallback: false,
                    })
                    .members
                    .push((bump.package.clone(), entry.severity));
            }

            if !has_substantive_entry {
                bystanders.push(bump.package.clone());
            }
        } else {
            let fallback_body = fallback_reason_text(bump);
            fallback_counter += 1;
            groups.insert(
                ChangeGroupKey::Fallback(fallback_counter),
                ChangeGroup {
                    body: fallback_body,
                    members: vec![(bump.package.clone(), bump.severity)],
                    is_fallback: true,
                },
            );
        }
    }

    if !bystanders.is_empty() {
        let names: Vec<String> = bystanders.iter().map(|p| format!("`{}`", p.display_name())).collect();
        body.push_str(&format!(
            "_Also bumped with no direct change of their own (released together via a fixed/linked group): {}._\n\n",
            names.join(", ")
        ));
    }

    body.push_str(&format!(
        "**{} change(s) across {} package(s):**\n\n",
        groups.len(),
        plan.bumps.len()
    ));

    for (key, group) in &groups {
        let inner_open = if group.has_major() { " open" } else { "" };

        if group.is_fallback {
            let (package, severity) = &group.members[0];
            body.push_str(&format!(
                "<details{}>\n<summary><b>{}</b> ({} {})</summary>\n\n",
                inner_open,
                package.display_name(),
                severity_emoji(*severity),
                severity
            ));
        } else {
            let affected: Vec<String> = group
                .members
                .iter()
                .map(|(package, _)| format!("<code>{}</code>", package.display_name()))
                .collect();
            body.push_str(&format!(
                "<details{}>\n<summary><b>{}</b> — affects {}</summary>\n\n",
                inner_open,
                change_group_label(key),
                affected.join(", ")
            ));
        }

        body.push_str(&group.body);
        if !group.body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str("\n</details>\n\n");
    }

    // 4. Instructions Footer
    let branch = opts.branch.as_deref().unwrap_or("callisto/version-packages");
    body.push_str("<details>\n<summary><b>Release Workflow Instructions</b></summary>\n\n");
    body.push_str("- **Merging this PR**: Merging into `main` will automatically publish all updated packages to their release registries.\n");
    body.push_str("- **Adding more changesets**: If additional changesets are pushed to `main`, Callisto will automatically re-calculate and update this PR.\n");
    body.push_str(&format!(
        "- **Manual edits**: You can make manual adjustments directly on the `{}` branch if needed.\n\n",
        branch
    ));
    body.push_str("</details>\n");

    Ok(ComposePrBodyReport {
        schema_version: SCHEMA_VERSION,
        body,
        diagnostics: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{PlannedBump, VersionPlan};
    use callisto_model::{BumpReason, PackageId, Severity, Version};

    #[test]
    fn test_compose_pr_body_snapshot() {
        let pkg_a = PackageId::parse("core-crate").unwrap();
        let pkg_b = PackageId::parse("@myorg/web-app").unwrap();

        let bump_a = PlannedBump {
            package: pkg_a,
            from: Version::semver(0, 1, 0),
            to: Version::semver(0, 2, 0),
            severity: Severity::Minor,
            governed_by: None,
            reason: Some(BumpReason::Changeset {
                changesets: vec!["swift-foxes-race".to_string()],
            }),
            writes: vec![],
        };

        let bump_b = PlannedBump {
            package: pkg_b,
            from: Version::semver(1, 0, 0),
            to: Version::semver(1, 0, 1),
            severity: Severity::Patch,
            governed_by: None,
            reason: Some(BumpReason::Changeset {
                changesets: vec!["swift-foxes-race".to_string()],
            }),
            writes: vec![],
        };

        let plan = VersionPlan {
            bumps: vec![bump_a, bump_b],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![],
            consumed_changesets: vec![std::path::PathBuf::from(".changeset/swift-foxes-race.md")],
            pre_state_update: None,
            delete_pre_json: None,
            pre_cursor_updates: vec![],
            observed_versions: std::collections::BTreeMap::new(),
            diagnostics: vec![],
        };

        let opts = PrBodyOptions::default();
        let report = render_pr_body_from_plan(&plan, &opts).unwrap();
        insta::assert_snapshot!(report.body);
    }

    #[test]
    fn test_compose_pr_body_custom_branch_override() {
        let plan = VersionPlan {
            bumps: vec![],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![],
            consumed_changesets: vec![],
            pre_state_update: None,
            delete_pre_json: None,
            pre_cursor_updates: vec![],
            observed_versions: std::collections::BTreeMap::new(),
            diagnostics: vec![],
        };

        let opts = PrBodyOptions {
            existing_body: None,
            branch: Some("custom-release-branch".to_string()),
        };
        let report = render_pr_body_from_plan(&plan, &opts).unwrap();
        assert!(report.body.contains("## Release Preview"));
    }

    fn plan_with_single_bump(reason: BumpReason) -> VersionPlan {
        VersionPlan {
            bumps: vec![PlannedBump {
                package: PackageId::parse("pkg-a").unwrap(),
                from: Version::semver(1, 0, 0),
                to: Version::semver(1, 1, 0),
                severity: Severity::Minor,
                governed_by: None,
                reason: Some(reason),
                writes: vec![],
            }],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![],
            consumed_changesets: vec![],
            pre_state_update: None,
            delete_pre_json: None,
            pre_cursor_updates: vec![],
            observed_versions: std::collections::BTreeMap::new(),
            diagnostics: vec![],
        }
    }

    /// For a plan with exactly one bump and no matching `changelog_writes`
    /// entry (so the sole change group is the package's own fallback
    /// block): confirms that group's body -- between its own `<summary>`
    /// and its closing `</details>` -- is non-empty.
    fn assert_release_reason_has_content(body: &str) {
        let changing_start = body
            .find("### 📦 What's Changing")
            .expect("must contain the What's Changing section");
        let after_heading = &body[changing_start..];
        let summary_end = after_heading
            .find("</summary>")
            .map(|i| i + "</summary>".len())
            .expect("must contain the fallback group's own </summary>");
        let content_area = &after_heading[summary_end..];
        let end = content_area.find("</details>").unwrap_or(content_area.len());
        let section = content_area[..end].trim();
        assert!(
            !section.is_empty(),
            "the fallback group's body must have non-whitespace content, got body: {body}"
        );
    }

    #[test]
    fn ac002_inference_fallback_is_non_empty() {
        let plan = plan_with_single_bump(BumpReason::Inference {
            commits: 3,
            remapped: false,
        });
        let report = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap();
        assert_release_reason_has_content(&report.body);
    }

    #[test]
    fn ac002_prerelease_fallback_is_non_empty() {
        let plan = plan_with_single_bump(BumpReason::PreRelease {
            tag: "alpha".to_string(),
        });
        let report = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap();
        assert_release_reason_has_content(&report.body);
    }

    #[test]
    fn ac002_new_group_member_fallback_is_non_empty() {
        let plan = plan_with_single_bump(BumpReason::NewGroupMember {
            group: callisto_model::GroupName("grp".to_string()),
        });
        let report = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap();
        assert_release_reason_has_content(&report.body);
    }

    #[test]
    fn ac001_render_section_takes_priority_over_fallback_when_both_available() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let bump = PlannedBump {
            package: pkg_a.clone(),
            from: Version::semver(1, 0, 0),
            to: Version::semver(1, 1, 0),
            severity: Severity::Minor,
            governed_by: None,
            reason: Some(BumpReason::Cascade {
                via: PackageId::parse("pkg-core").unwrap(),
                dep_kind: callisto_model::DepKind::Runtime,
                spec: "^1.0.0".to_string(),
                dependency_to: Version::semver(2, 0, 0),
            }),
            writes: vec![],
        };

        let input = callisto_changelog::ChangelogInput {
            package: pkg_a.clone(),
            from: Version::semver(1, 0, 0),
            to: Some(Version::semver(1, 1, 0)),
            entries: vec![callisto_changelog::ChangelogEntry {
                severity: Severity::Minor,
                source: callisto_changelog::ChangeSource::Changeset {
                    filename: "swift-foxes.md".to_string(),
                    summary: "A real changeset summary.".to_string(),
                },
            }],
        };
        // Confirms render_section would succeed against this input (the
        // real gate `render_pr_body_from_plan` checks) without asserting on
        // its whole-section output, which includes a "## {version}" heading
        // no longer meaningful once entries are regrouped across packages.
        callisto_changelog::render_section(&input).unwrap();

        let plan = VersionPlan {
            bumps: vec![bump],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![crate::plan::ChangelogWrite {
                changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
                input,
            }],
            consumed_changesets: vec![],
            pre_state_update: None,
            delete_pre_json: None,
            pre_cursor_updates: vec![],
            observed_versions: std::collections::BTreeMap::new(),
            diagnostics: vec![],
        };

        let report = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap();

        assert!(
            report.body.contains("A real changeset summary."),
            "body must contain the real changeset entry's text; body: {}",
            report.body
        );
        assert!(
            report.body.contains("<code>pkg-a</code>"),
            "body must list pkg-a as affected by the change group; body: {}",
            report.body
        );
        assert!(
            !report.body.contains("Automatic dependency cascade triggered by"),
            "body must NOT contain the fallback Cascade text when render_section succeeded; body: {}",
            report.body
        );
    }

    #[test]
    fn ac007_empty_entries_falls_back_and_other_package_renders_normally() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let pkg_b = PackageId::parse("pkg-b").unwrap();

        let bump_a = PlannedBump {
            package: pkg_a.clone(),
            from: Version::semver(1, 0, 0),
            to: Version::semver(1, 1, 0),
            severity: Severity::Minor,
            governed_by: None,
            reason: Some(BumpReason::Cascade {
                via: PackageId::parse("pkg-core").unwrap(),
                dep_kind: callisto_model::DepKind::Runtime,
                spec: "^1.0.0".to_string(),
                dependency_to: Version::semver(2, 0, 0),
            }),
            writes: vec![],
        };
        let bump_b = PlannedBump {
            package: pkg_b.clone(),
            from: Version::semver(1, 0, 0),
            to: Version::semver(1, 0, 1),
            severity: Severity::Patch,
            governed_by: None,
            reason: Some(BumpReason::Changeset {
                changesets: vec!["fix-1".to_string()],
            }),
            writes: vec![],
        };

        let good_input = callisto_changelog::ChangelogInput {
            package: pkg_b.clone(),
            from: Version::semver(1, 0, 0),
            to: Some(Version::semver(1, 0, 1)),
            entries: vec![callisto_changelog::ChangelogEntry {
                severity: Severity::Patch,
                source: callisto_changelog::ChangeSource::Changeset {
                    filename: "fix-1.md".to_string(),
                    summary: "A fix.".to_string(),
                },
            }],
        };
        callisto_changelog::render_section(&good_input).unwrap();

        let plan = VersionPlan {
            bumps: vec![bump_a, bump_b],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![
                crate::plan::ChangelogWrite {
                    changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
                    input: callisto_changelog::ChangelogInput {
                        package: pkg_a.clone(),
                        from: Version::semver(1, 0, 0),
                        to: Some(Version::semver(1, 1, 0)),
                        entries: vec![],
                    },
                },
                crate::plan::ChangelogWrite {
                    changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
                    input: good_input,
                },
            ],
            consumed_changesets: vec![],
            pre_state_update: None,
            delete_pre_json: None,
            pre_cursor_updates: vec![],
            observed_versions: std::collections::BTreeMap::new(),
            diagnostics: vec![],
        };

        let report = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap();
        assert!(
            report.body.contains("Automatic dependency cascade triggered by"),
            "pkg-a must use the AC-002 fallback text since its ChangelogWrite has empty entries; body: {}",
            report.body
        );
        assert!(
            report.body.contains("A fix."),
            "pkg-b must still render normally via its real changeset entry; body: {}",
            report.body
        );
        assert!(
            report.body.contains("<code>pkg-b</code>"),
            "pkg-b must be listed as affected by its change group; body: {}",
            report.body
        );
    }

    #[test]
    fn ac007_severity_none_entry_falls_back() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let bump_a = PlannedBump {
            package: pkg_a.clone(),
            from: Version::semver(1, 0, 0),
            to: Version::semver(1, 1, 0),
            severity: Severity::Minor,
            governed_by: None,
            reason: Some(BumpReason::Cascade {
                via: PackageId::parse("pkg-core").unwrap(),
                dep_kind: callisto_model::DepKind::Runtime,
                spec: "^1.0.0".to_string(),
                dependency_to: Version::semver(2, 0, 0),
            }),
            writes: vec![],
        };
        let plan = VersionPlan {
            bumps: vec![bump_a],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![crate::plan::ChangelogWrite {
                changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
                input: callisto_changelog::ChangelogInput {
                    package: pkg_a.clone(),
                    from: Version::semver(1, 0, 0),
                    to: Some(Version::semver(1, 1, 0)),
                    entries: vec![callisto_changelog::ChangelogEntry {
                        severity: Severity::None,
                        source: callisto_changelog::ChangeSource::Changeset {
                            filename: "x.md".to_string(),
                            summary: "whatever".to_string(),
                        },
                    }],
                },
            }],
            consumed_changesets: vec![],
            pre_state_update: None,
            delete_pre_json: None,
            pre_cursor_updates: vec![],
            observed_versions: std::collections::BTreeMap::new(),
            diagnostics: vec![],
        };

        let report = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap();
        assert!(
            report.body.contains("Automatic dependency cascade triggered by"),
            "body: {}",
            report.body
        );
    }

    #[test]
    fn ac007_empty_entries_and_no_reason_uses_no_changelog_entries_literal() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let bump_a = PlannedBump {
            package: pkg_a.clone(),
            from: Version::semver(1, 0, 0),
            to: Version::semver(1, 1, 0),
            severity: Severity::Minor,
            governed_by: None,
            reason: None,
            writes: vec![],
        };
        let plan = VersionPlan {
            bumps: vec![bump_a],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![crate::plan::ChangelogWrite {
                changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
                input: callisto_changelog::ChangelogInput {
                    package: pkg_a.clone(),
                    from: Version::semver(1, 0, 0),
                    to: Some(Version::semver(1, 1, 0)),
                    entries: vec![],
                },
            }],
            consumed_changesets: vec![],
            pre_state_update: None,
            delete_pre_json: None,
            pre_cursor_updates: vec![],
            observed_versions: std::collections::BTreeMap::new(),
            diagnostics: vec![],
        };

        let report = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap();
        assert!(
            report.body.contains("_No changelog entries available._"),
            "body: {}",
            report.body
        );
    }

    /// The headline fix this module exists for: a single `.changeset/*.md`
    /// file names several packages (§6.1's fan-out model) and its summary
    /// is copied verbatim into each one's own `ChangelogEntry`. Before this
    /// change, the PR body repeated that summary once per package; it must
    /// now render exactly once, with every affected package listed in its
    /// `<summary>`.
    #[test]
    fn shared_changeset_across_packages_renders_exactly_once() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let pkg_b = PackageId::parse("pkg-b").unwrap();
        let shared_summary = "**Delete unused abstractions with zero real callers**\n\nDetail text.";

        let make_write = |package: PackageId| crate::plan::ChangelogWrite {
            changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
            input: callisto_changelog::ChangelogInput {
                package: package.clone(),
                from: Version::semver(1, 0, 0),
                to: Some(Version::semver(1, 1, 0)),
                entries: vec![callisto_changelog::ChangelogEntry {
                    severity: Severity::Minor,
                    source: callisto_changelog::ChangeSource::Changeset {
                        filename: "delete-dead-abstractions.md".to_string(),
                        summary: shared_summary.to_string(),
                    },
                }],
            },
        };

        let plan = VersionPlan {
            bumps: vec![
                PlannedBump {
                    package: pkg_a.clone(),
                    from: Version::semver(1, 0, 0),
                    to: Version::semver(1, 1, 0),
                    severity: Severity::Minor,
                    governed_by: None,
                    reason: Some(BumpReason::Changeset {
                        changesets: vec!["delete-dead-abstractions".to_string()],
                    }),
                    writes: vec![],
                },
                PlannedBump {
                    package: pkg_b.clone(),
                    from: Version::semver(1, 0, 0),
                    to: Version::semver(1, 1, 0),
                    severity: Severity::Minor,
                    governed_by: None,
                    reason: Some(BumpReason::Changeset {
                        changesets: vec!["delete-dead-abstractions".to_string()],
                    }),
                    writes: vec![],
                },
            ],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![make_write(pkg_a), make_write(pkg_b)],
            consumed_changesets: vec![],
            pre_state_update: None,
            delete_pre_json: None,
            pre_cursor_updates: vec![],
            observed_versions: std::collections::BTreeMap::new(),
            diagnostics: vec![],
        };

        let report = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap();

        assert_eq!(
            report
                .body
                .matches("Delete unused abstractions with zero real callers")
                .count(),
            1,
            "the shared changeset's text must render exactly once, not once per package; body: {}",
            report.body
        );
        assert!(report.body.contains("<code>pkg-a</code>"), "body: {}", report.body);
        assert!(report.body.contains("<code>pkg-b</code>"), "body: {}", report.body);
        assert!(
            report.body.contains("`delete-dead-abstractions`"),
            "the group's summary must name its changeset; body: {}",
            report.body
        );
    }

    /// The outer "What's Changing" wrapper defaults closed for a routine
    /// "What's Changing" has no outer collapsible: the list of what changed
    /// (each change's own `<summary>` line) must always be directly visible
    /// with no click required, even for a routine minor-only release --
    /// only each change's own *body* text collapses. An earlier revision
    /// wrapped the whole section in one more collapsible on top of that,
    /// which made the list itself invisible by default; that wrapper is
    /// gone.
    #[test]
    fn whats_changing_list_is_never_hidden_behind_an_outer_collapsible() {
        let plan = plan_with_single_bump(BumpReason::Changeset {
            changesets: vec!["x".to_string()],
        });
        let report = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap();
        let changing_start = report
            .body
            .find("### 📦 What's Changing")
            .expect("must contain the What's Changing heading");
        assert!(
            !report.body[changing_start..].contains("<details>\n<summary><b>1 change"),
            "the change list must not be wrapped in its own collapsible; body: {}",
            report.body
        );
        assert!(
            report.body.contains("**1 change(s) across 1 package(s):**"),
            "a plain, always-visible count line replaces the old collapsible wrapper; body: {}",
            report.body
        );
    }

    /// Each change's own block still defaults closed for a routine
    /// minor/patch bump and open when it contains a major bump, so a
    /// breaking change's detail is never hidden behind an extra click by
    /// default.
    #[test]
    fn individual_change_block_opens_only_when_it_contains_a_major_bump() {
        let minor_only = plan_with_single_bump(BumpReason::Changeset {
            changesets: vec!["x".to_string()],
        });
        let minor_report = render_pr_body_from_plan(&minor_only, &PrBodyOptions::default()).unwrap();
        assert!(
            minor_report.body.contains("<details>\n<summary><b>pkg-a</b>"),
            "a minor-only change's block must default closed; body: {}",
            minor_report.body
        );

        let mut major_plan = plan_with_single_bump(BumpReason::Changeset {
            changesets: vec!["y".to_string()],
        });
        major_plan.bumps[0].severity = Severity::Major;
        let major_report = render_pr_body_from_plan(&major_plan, &PrBodyOptions::default()).unwrap();
        assert!(
            major_report.body.contains("<details open>\n<summary><b>pkg-a</b>"),
            "a change containing a major bump must default its own block open; body: {}",
            major_report.body
        );
    }

    /// A package bumped only because it shares a fixed/linked group with a
    /// package that has a real change -- no substantive entry of its own --
    /// is listed in a short note instead of getting its own empty-looking
    /// collapsible block.
    #[test]
    fn bystander_package_gets_a_note_not_an_empty_block() {
        let pkg_a = PackageId::parse("pkg-a").unwrap();
        let bump_a = PlannedBump {
            package: pkg_a.clone(),
            from: Version::semver(1, 0, 0),
            to: Version::semver(1, 1, 0),
            severity: Severity::Minor,
            governed_by: None,
            reason: Some(BumpReason::FixedGroupUnion {
                group: callisto_model::GroupName("workspace".to_string()),
            }),
            writes: vec![],
        };
        let plan = VersionPlan {
            bumps: vec![bump_a],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![crate::plan::ChangelogWrite {
                changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
                input: callisto_changelog::ChangelogInput {
                    package: pkg_a.clone(),
                    from: Version::semver(1, 0, 0),
                    to: Some(Version::semver(1, 1, 0)),
                    entries: vec![callisto_changelog::ChangelogEntry {
                        severity: Severity::Minor,
                        source: callisto_changelog::ChangeSource::GroupUnion {
                            group: callisto_model::GroupName("workspace".to_string()),
                            kind: callisto_model::GroupKind::Fixed,
                        },
                    }],
                },
            }],
            consumed_changesets: vec![],
            pre_state_update: None,
            delete_pre_json: None,
            pre_cursor_updates: vec![],
            observed_versions: std::collections::BTreeMap::new(),
            diagnostics: vec![],
        };

        let report = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap();
        assert!(
            report.body.contains("Also bumped with no direct change of their own"),
            "body: {}",
            report.body
        );
        assert!(report.body.contains("`pkg-a`"), "body: {}", report.body);
        assert_eq!(
            report.body.matches("<details>\n<summary><b>pkg-a</b>").count(),
            0,
            "a bystander package must not also get its own collapsible block; body: {}",
            report.body
        );
    }

    #[test]
    fn suggested_pr_labels_line_is_never_rendered() {
        let plan = plan_with_single_bump(BumpReason::Changeset {
            changesets: vec!["x".to_string()],
        });
        let report = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap();
        assert!(
            !report.body.contains("Suggested PR Label"),
            "the label line duplicates GitHub's own label UI and is never merely \"suggested\" \
             (the caller always applies it); body: {}",
            report.body
        );
    }
}
