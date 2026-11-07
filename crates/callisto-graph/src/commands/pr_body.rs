use callisto_model::{BumpReason, CommandRunner, ComposePrBodyReport, Severity, SCHEMA_VERSION};

use crate::commands::version::plan_version;
use crate::error::GraphError;
use crate::infer::SeverityInference;
use crate::release_preview::{
    build_release_preview, split_summary, Change, ChangeKind, Finding, ReleasePreview, FINDING_MAJOR_BUMP,
};
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

/// One change's bullet text. `None` for a change with nothing to say.
fn entry_text(kind: &ChangeKind) -> Option<String> {
    match kind {
        ChangeKind::Changeset { summary, .. } => Some(summary.trim().to_owned()),
        ChangeKind::Commit { subject, short_sha, .. } => Some(format!("{subject} ({short_sha})")),
        ChangeKind::DependencyUpdate { dependency, to } => Some(format!("`{dependency}` → `{to}`")),
        ChangeKind::PeerEscalation { dependency, to } => {
            Some(format!("Peer dependency `{dependency}` requires `{to}`"))
        }
        ChangeKind::Fallback { reason } => Some(fallback_reason_text(reason)),
    }
}

/// A short, stable label for a change's `<summary>`, from its identity and never from its free-text body.
fn change_label(kind: &ChangeKind) -> String {
    match kind {
        ChangeKind::Changeset { filename, .. } => {
            format!("`{}`", filename.strip_suffix(".md").unwrap_or(filename))
        }
        ChangeKind::Commit { sha, .. } => format!("Commit `{}`", &sha[..sha.len().min(7)]),
        ChangeKind::PeerEscalation { dependency, to } => format!("Peer dependency `{dependency}` → `{to}`"),
        ChangeKind::DependencyUpdate { dependency, to } => format!("Dependency `{dependency}` → `{to}`"),
        ChangeKind::Fallback { .. } => unreachable!("Fallback changes render via their sole member, not this label"),
    }
}

/// Text for a package with no renderable changelog entries, from why it bumps.
fn fallback_reason_text(reason: &Option<BumpReason>) -> String {
    match reason {
        Some(reason) => match reason {
            BumpReason::Changeset { changesets } => {
                format!("- Associated changesets: `{}`\n", changesets.join("`, `"))
            }
            BumpReason::Cascade {
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
            BumpReason::PeerEscalation { via, spec } => {
                format!(
                    "- Peer dependency escalation triggered by `{}` (spec `{}`).\n",
                    via.display_name(),
                    spec
                )
            }
            BumpReason::FixedGroupUnion { group } => {
                format!("- Fixed group synchronization for group `{}`.\n", group.as_str())
            }
            BumpReason::LinkedGroupUnion { group } => {
                format!("- Linked group version alignment for group `{}`.\n", group.as_str())
            }
            BumpReason::Inference { commits, .. } => {
                format!("- Inferred from {commits} commit(s) since the last release.\n")
            }
            BumpReason::PreRelease { tag } => {
                format!("- Pre-release finalization (tag `{tag}`).\n")
            }
            BumpReason::NewGroupMember { group } => {
                format!("- New member of group `{}`.\n", group.as_str())
            }
            _ => "_No changelog entries available._\n".to_string(),
        },
        None => "_No changelog entries available._\n".to_string(),
    }
}

fn reason_cell(reason: &Option<BumpReason>) -> String {
    match reason {
        Some(BumpReason::Changeset { changesets }) => {
            format!("Changeset (`{}`)", changesets.join("`, `"))
        }
        Some(BumpReason::Cascade { via, .. }) => {
            format!("Cascade from `{}`", via.display_name())
        }
        Some(BumpReason::PeerEscalation { via, .. }) => {
            format!("Peer escalation from `{}`", via.display_name())
        }
        Some(BumpReason::FixedGroupUnion { group }) => {
            format!("Fixed group `{}`", group.as_str())
        }
        Some(BumpReason::LinkedGroupUnion { group }) => {
            format!("Linked group `{}`", group.as_str())
        }
        Some(BumpReason::Inference { commits, .. }) => {
            format!("Inferred ({commits} commits)")
        }
        Some(BumpReason::PreRelease { tag }) => {
            format!("Prerelease (`{tag}`)")
        }
        Some(BumpReason::NewGroupMember { group }) => {
            format!("New member (`{}`)", group.as_str())
        }
        _ => "Package bump".to_string(),
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
    Ok(ComposePrBodyReport {
        schema_version: SCHEMA_VERSION,
        body: render_pr_body(&build_release_preview(plan), opts),
        diagnostics: Vec::new(),
    })
}

fn workflow_instructions(branch: &str) -> String {
    let mut out = String::new();
    out.push_str("<details>\n<summary><b>Release Workflow Instructions</b></summary>\n\n");
    out.push_str("- **Merging this PR**: Merging into `main` will automatically publish all updated packages to their release registries.\n");
    out.push_str("- **Adding more changesets**: If additional changesets are pushed to `main`, Callisto will automatically re-calculate and update this PR.\n");
    out.push_str(&format!(
        "- **Manual edits**: You can make manual adjustments directly on the `{branch}` branch if needed.\n\n"
    ));
    out.push_str("</details>\n");
    out
}

fn render_pr_body(notes: &ReleasePreview, opts: &PrBodyOptions) -> String {
    let mut body = String::new();

    // Preserve custom user notes above ## Release Preview if an existing body was passed
    if let Some(ref existing) = opts.existing_body
        && let Some((prefix, _)) = existing.split_once("## Release Preview")
        && !prefix.trim().is_empty()
    {
        body.push_str(prefix);
        if !prefix.ends_with('\n') {
            body.push('\n');
        }
    }

    body.push_str("## Release Preview\n\n");
    body.push_str("This automated PR was generated by [Callisto](https://github.com/orin-dx/callisto). Merging this PR will publish the updated packages to their release targets.\n\n");

    let branch = opts.branch.as_deref().unwrap_or("callisto/version-packages");

    if notes.packages.is_empty() {
        body.push_str("> [!NOTE]\n");
        body.push_str("> **No pending changesets found.** Workspace packages are currently up to date.\n\n");
        body.push_str(&workflow_instructions(branch));
        return body;
    }

    // Major-first (the order `notes.packages` already has): a reviewer's real question is "is anything breaking,"
    // and that must be answerable from the top of the table without scanning every row.
    body.push_str("### 📊 Release Summary\n\n");
    body.push_str("| Package | Ecosystem | Current | Target | Bump | Reason |\n");
    body.push_str("| :--- | :--- | :--- | :--- | :--- | :--- |\n");
    for package in &notes.packages {
        body.push_str(&format!(
            "| `{}` | `{}` | `{}` | **`{}`** | {} `{}` | {} |\n",
            package.name,
            package.ecosystem,
            package.from,
            package.to,
            severity_emoji(package.severity),
            package.severity,
            reason_cell(&package.reason)
        ));
    }
    body.push('\n');

    body.push_str(&callout(&notes.findings, notes.packages.len()));

    // Grouped by underlying change, not by package, so a changeset naming several packages renders once.
    body.push_str("### 📦 What's Changing\n\n");

    if !notes.bystanders.is_empty() {
        let names: Vec<String> = notes.bystanders.iter().map(|name| format!("`{name}`")).collect();
        body.push_str(&format!(
            "_Also bumped with no direct change of their own (released together via a fixed/linked group): {}._\n\n",
            names.join(", ")
        ));
    }

    body.push_str(&format!(
        "**{} change(s) across {} package(s):**\n\n",
        notes.changes.len(),
        notes.packages.len()
    ));

    for level in [Severity::Major, Severity::Minor, Severity::Patch] {
        let in_level: Vec<&Change> = notes
            .changes
            .iter()
            .filter(|change| change_level(change) == level)
            .collect();
        if in_level.is_empty() {
            continue;
        }
        body.push_str(&format!("#### {} {}\n\n", severity_emoji(level), level_title(level)));
        let mut after_bullet = false;
        for change in in_level {
            after_bullet = render_change(&mut body, change, after_bullet);
        }
        if after_bullet {
            body.push('\n');
        }
    }

    body.push_str(&workflow_instructions(branch));
    body
}

/// Changes are grouped by their highest bump; a bump of `none` reads as a patch.
fn change_level(change: &Change) -> Severity {
    match change.max_severity() {
        Severity::None => Severity::Patch,
        level => level,
    }
}

fn level_title(level: Severity) -> &'static str {
    match level {
        Severity::Major => "Major",
        Severity::Minor => "Minor",
        _ => "Patch",
    }
}

/// Appends one change and returns whether it was a bullet, so the caller can keep consecutive bullets in one list.
fn render_change(body: &mut String, change: &Change, after_bullet: bool) -> bool {
    let open = if change.max_severity() == Severity::Major {
        " open"
    } else {
        ""
    };

    if let ChangeKind::Fallback { .. } = change.kind {
        if after_bullet {
            body.push('\n');
        }
        let member = &change.members[0];
        body.push_str(&format!(
            "<details{open}>\n<summary><b>{}</b> ({} {})</summary>\n\n",
            member.package,
            severity_emoji(member.severity),
            member.severity
        ));
        push_detail(body, &entry_text(&change.kind).unwrap_or_default());
        return false;
    }

    let (title, detail) = match &change.kind {
        ChangeKind::Changeset { summary, .. } => {
            let split = split_summary(summary);
            match split.headline {
                Some(headline) => (headline, split.detail),
                None => (change_label(&change.kind), split.detail),
            }
        }
        kind => (change_label(kind), entry_text(kind).unwrap_or_default()),
    };
    let affected: Vec<String> = change
        .members
        .iter()
        .map(|member| format!("<code>{}</code>", member.package))
        .collect();

    if detail.is_empty() {
        body.push_str(&format!("- **{title}** — affects {}\n", affected.join(", ")));
        return true;
    }
    if after_bullet {
        body.push('\n');
    }
    body.push_str(&format!(
        "<details{open}>\n<summary><b>{}</b> — affects {}</summary>\n\n",
        summary_html(&title),
        affected.join(", ")
    ));
    push_detail(body, &detail);
    false
}

/// Markdown is not interpreted inside a raw-HTML `<summary>`, so code spans become `<code>` and everything else is
/// escaped; otherwise a title such as ``Fix `<T>` `` would lose its `<T>` as an unknown tag.
fn summary_html(text: &str) -> String {
    fn escape(text: &str) -> String {
        text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let Some(len) = rest[open + 1..].find('`') else {
            break;
        };
        out.push_str(&escape(&rest[..open]));
        out.push_str(&format!("<code>{}</code>", escape(&rest[open + 1..open + 1 + len])));
        rest = &rest[open + len + 2..];
    }
    out.push_str(&escape(rest));
    out
}

fn push_detail(body: &mut String, text: &str) {
    body.push_str(text);
    if !text.ends_with('\n') {
        body.push('\n');
    }
    body.push_str("\n</details>\n\n");
}

/// The callout that opens the release PR body.
fn callout(findings: &[Finding], package_count: usize) -> String {
    if let Some(major) = findings.iter().find(|finding| finding.id == FINDING_MAJOR_BUMP) {
        return format!(
            "> [!WARNING]\n> **Major Version Bumps Detected**: {}\n\n",
            major.message
        );
    }
    let verdict = findings
        .first()
        .map(|finding| finding.message.as_str())
        .unwrap_or_default();
    format!("> [!NOTE]\n> **{package_count} package(s)** queued for versioning. {verdict}\n\n")
}

#[cfg(test)]
fn review_callout(bumps: &[crate::plan::PlannedBump]) -> String {
    callout(&crate::release_preview::assess(bumps), bumps.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{PlannedBump, VersionPlan};
    use callisto_model::{BumpReason, PackageId, Severity, Version};

    #[test]
    fn a_summary_title_keeps_code_spans_and_escapes_the_rest() {
        assert_eq!(
            summary_html("Fix `<T>` & more"),
            "Fix <code>&lt;T&gt;</code> &amp; more"
        );
        assert_eq!(summary_html("a < b `unclosed"), "a &lt; b `unclosed");
        assert_eq!(summary_html("`a` and `b`"), "<code>a</code> and <code>b</code>");
    }

    #[test]
    fn changes_are_grouped_by_bump_and_titled_by_headline() {
        let write = |package: &str, severity: Severity, filename: &str, summary: &str| {
            let package = PackageId::parse(package).unwrap();
            let bump = PlannedBump {
                package: package.clone(),
                from: Version::semver(1, 0, 0),
                to: Version::semver(2, 0, 0),
                severity,
                governed_by: None,
                reason: None,
                writes: vec![],
            };
            let write = crate::plan::ChangelogWrite {
                changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
                input: crate::changelog::ChangelogInput {
                    package,
                    from: Version::semver(1, 0, 0),
                    to: Some(Version::semver(2, 0, 0)),
                    entries: vec![crate::changelog::ChangelogEntry {
                        severity,
                        source: crate::changelog::ChangeSource::Changeset {
                            filename: filename.to_owned(),
                            summary: summary.to_owned(),
                        },
                    }],
                },
            };
            (bump, write)
        };
        let (minor_bump, minor_write) = write("pkg-b", Severity::Minor, "b.md", "Adds a flag.");
        let (major_bump, major_write) = write(
            "pkg-a",
            Severity::Major,
            "a.md",
            "Drops the old API.\n\nCallers must now pass a config.",
        );
        let plan = VersionPlan {
            bumps: vec![minor_bump, major_bump],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![minor_write, major_write],
            consumed_changesets: vec![],
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };

        let body = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap().body;

        let major = body.find("#### 🔴 Major").expect("major heading");
        let minor = body.find("#### 🟡 Minor").expect("minor heading");
        assert!(major < minor, "{body}");
        assert!(!body.contains("#### 🟢 Patch"), "{body}");
        assert!(
            body.contains(
                "<details open>\n<summary><b>Drops the old API.</b> — affects <code>pkg-a</code></summary>\n\nCallers must now pass a config.\n\n</details>"
            ),
            "{body}"
        );
        assert!(
            body.contains("- **Adds a flag.** — affects <code>pkg-b</code>\n\n"),
            "{body}"
        );
        assert!(
            !body.contains("`a`") && !body.contains("`b`"),
            "titles are headlines, not filenames: {body}"
        );
    }

    fn bump(from: Version, to: Version, severity: Severity) -> PlannedBump {
        PlannedBump {
            package: PackageId::parse("core-crate").unwrap(),
            from,
            to,
            severity,
            governed_by: None,
            reason: None,
            writes: vec![],
        }
    }

    #[test]
    fn a_pre_one_minor_bump_is_not_called_backward_compatible() {
        let callout = review_callout(&[bump(
            Version::semver(0, 11, 0),
            Version::semver(0, 12, 0),
            Severity::Minor,
        )]);
        assert!(!callout.contains("backward compatible"), "{callout}");
        assert!(
            callout.contains("below 1.0.0 can include breaking changes"),
            "{callout}"
        );
    }

    #[test]
    fn stable_minor_and_patch_bumps_are_called_backward_compatible() {
        let callout = review_callout(&[
            bump(Version::semver(1, 2, 0), Version::semver(1, 3, 0), Severity::Minor),
            bump(Version::semver(0, 4, 1), Version::semver(0, 4, 2), Severity::Patch),
        ]);
        assert!(
            callout.contains("2 package(s)") && callout.contains("All changes are backward compatible."),
            "{callout}"
        );
    }

    #[test]
    fn a_major_bump_warns() {
        let callout = review_callout(&[bump(
            Version::semver(1, 0, 0),
            Version::semver(2, 0, 0),
            Severity::Major,
        )]);
        assert!(callout.starts_with("> [!WARNING]"), "{callout}");
    }

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
            writes: vec![crate::plan::VersionWriteTarget::Manifest(
                "core-crate/Cargo.toml".into(),
            )],
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
            writes: vec![crate::plan::VersionWriteTarget::Manifest("web-app/package.json".into())],
        };

        let plan = VersionPlan {
            bumps: vec![bump_a, bump_b],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![],
            consumed_changesets: vec![std::path::PathBuf::from(".changeset/swift-foxes-race.md")],
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
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
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
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
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
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
    fn inference_fallback_is_non_empty() {
        let plan = plan_with_single_bump(BumpReason::Inference {
            commits: 3,
            remapped: false,
        });
        let report = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap();
        assert_release_reason_has_content(&report.body);
    }

    #[test]
    fn prerelease_fallback_is_non_empty() {
        let plan = plan_with_single_bump(BumpReason::PreRelease {
            tag: "alpha".to_string(),
        });
        let report = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap();
        assert_release_reason_has_content(&report.body);
    }

    #[test]
    fn new_group_member_fallback_is_non_empty() {
        let plan = plan_with_single_bump(BumpReason::NewGroupMember {
            group: callisto_model::GroupName("grp".to_string()),
        });
        let report = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap();
        assert_release_reason_has_content(&report.body);
    }

    #[test]
    fn render_section_takes_priority_over_fallback_when_both_available() {
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

        let input = crate::changelog::ChangelogInput {
            package: pkg_a.clone(),
            from: Version::semver(1, 0, 0),
            to: Some(Version::semver(1, 1, 0)),
            entries: vec![crate::changelog::ChangelogEntry {
                severity: Severity::Minor,
                source: crate::changelog::ChangeSource::Changeset {
                    filename: "swift-foxes.md".to_string(),
                    summary: "A real changeset summary.".to_string(),
                },
            }],
        };
        // Confirms render_section would succeed against this input (the
        // real gate `render_pr_body_from_plan` checks) without asserting on
        // its whole-section output, which includes a "## {version}" heading
        // no longer meaningful once entries are regrouped across packages.
        crate::changelog::render_section(&input).unwrap();

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
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
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
    fn empty_entries_falls_back_and_other_package_renders_normally() {
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

        let good_input = crate::changelog::ChangelogInput {
            package: pkg_b.clone(),
            from: Version::semver(1, 0, 0),
            to: Some(Version::semver(1, 0, 1)),
            entries: vec![crate::changelog::ChangelogEntry {
                severity: Severity::Patch,
                source: crate::changelog::ChangeSource::Changeset {
                    filename: "fix-1.md".to_string(),
                    summary: "A fix.".to_string(),
                },
            }],
        };
        crate::changelog::render_section(&good_input).unwrap();

        let plan = VersionPlan {
            bumps: vec![bump_a, bump_b],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![
                crate::plan::ChangelogWrite {
                    changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
                    input: crate::changelog::ChangelogInput {
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
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };

        let report = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap();
        assert!(
            report.body.contains("Automatic dependency cascade triggered by"),
            "pkg-a must use the fallback text since its ChangelogWrite has empty entries; body: {}",
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
    fn severity_none_entry_falls_back() {
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
                input: crate::changelog::ChangelogInput {
                    package: pkg_a.clone(),
                    from: Version::semver(1, 0, 0),
                    to: Some(Version::semver(1, 1, 0)),
                    entries: vec![crate::changelog::ChangelogEntry {
                        severity: Severity::None,
                        source: crate::changelog::ChangeSource::Changeset {
                            filename: "x.md".to_string(),
                            summary: "whatever".to_string(),
                        },
                    }],
                },
            }],
            consumed_changesets: vec![],
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
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
    fn empty_entries_and_no_reason_uses_no_changelog_entries_literal() {
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
                input: crate::changelog::ChangelogInput {
                    package: pkg_a.clone(),
                    from: Version::semver(1, 0, 0),
                    to: Some(Version::semver(1, 1, 0)),
                    entries: vec![],
                },
            }],
            consumed_changesets: vec![],
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
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
    /// file names several packages (fan-out) and its summary
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
            input: crate::changelog::ChangelogInput {
                package: package.clone(),
                from: Version::semver(1, 0, 0),
                to: Some(Version::semver(1, 1, 0)),
                entries: vec![crate::changelog::ChangelogEntry {
                    severity: Severity::Minor,
                    source: crate::changelog::ChangeSource::Changeset {
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
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
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
                input: crate::changelog::ChangelogInput {
                    package: pkg_a.clone(),
                    from: Version::semver(1, 0, 0),
                    to: Some(Version::semver(1, 1, 0)),
                    entries: vec![crate::changelog::ChangelogEntry {
                        severity: Severity::Minor,
                        source: crate::changelog::ChangeSource::GroupUnion {
                            group: callisto_model::GroupName("workspace".to_string()),
                            kind: callisto_model::GroupKind::Fixed,
                        },
                    }],
                },
            }],
            consumed_changesets: vec![],
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
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

    /// A reviewer's real question is "is anything breaking" -- that must be
    /// answerable from the top of both the summary table and the change
    /// list, not by scanning past every row whose package name happens to
    /// sort earlier. `pkg-z` (patch) is declared first and `pkg-a` (major)
    /// last, deliberately the opposite of alphabetical/plan order, so this
    /// only passes if severity -- not declaration order -- drives the sort.
    #[test]
    fn major_bumps_are_ordered_first_in_the_table_and_change_list() {
        let make_bump = |name: &str, severity: Severity, changeset: &str| PlannedBump {
            package: PackageId::parse(name).unwrap(),
            from: Version::semver(1, 0, 0),
            to: Version::semver(1, 1, 0),
            severity,
            governed_by: None,
            reason: Some(BumpReason::Changeset {
                changesets: vec![changeset.to_string()],
            }),
            writes: vec![],
        };

        let plan = VersionPlan {
            bumps: vec![
                make_bump("pkg-z", Severity::Patch, "z-fix"),
                make_bump("pkg-m", Severity::Minor, "m-feature"),
                make_bump("pkg-a", Severity::Major, "a-breaking"),
            ],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![],
            consumed_changesets: vec![],
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };

        let report = render_pr_body_from_plan(&plan, &PrBodyOptions::default()).unwrap();

        let table_a = report.body.find("`pkg-a`").expect("pkg-a must appear in the table");
        let table_m = report.body.find("`pkg-m`").expect("pkg-m must appear in the table");
        let table_z = report.body.find("`pkg-z`").expect("pkg-z must appear in the table");
        assert!(
            table_a < table_m && table_m < table_z,
            "table rows must be ordered major, minor, patch regardless of declaration order; body: {}",
            report.body
        );

        let changing_start = report.body.find("### 📦 What's Changing").unwrap();
        let list_a = report.body[changing_start..]
            .find("a-breaking")
            .expect("a-breaking must appear in the change list");
        let list_m = report.body[changing_start..]
            .find("m-feature")
            .expect("m-feature must appear in the change list");
        let list_z = report.body[changing_start..]
            .find("z-fix")
            .expect("z-fix must appear in the change list");
        assert!(
            list_a < list_m && list_m < list_z,
            "the change list must also put the major-containing change first; body: {}",
            report.body
        );
    }
}
