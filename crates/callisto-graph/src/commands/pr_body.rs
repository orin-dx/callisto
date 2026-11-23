use callisto_model::{BumpReason, CommandRunner, ComposePrBodyReport, Severity, SCHEMA_VERSION};

use crate::changelog::render::{
    commit_bullet, dependency_bullet, indent_changeset_summary, peer_bullet, DEPENDENCY_HEADING,
};
use crate::commands::version::plan_version;
use crate::error::GraphError;
use crate::infer::SeverityInference;
use crate::release_preview::{
    Change, ChangeKind, Finding, FindingLevel, PackageRelease, PublishOutlook, PublishTargets, ReleasePreview,
    FINDING_ACTION_REQUIRED, FINDING_BREAKING_BUMP_MISMATCH, FINDING_BREAKING_MARKER, FINDING_MAJOR_BUMP,
    FINDING_UNRELEASED_CHANGES,
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

/// A changeset's label when its summary is empty, from its filename.
fn changeset_label(filename: &str) -> String {
    format!("`{}`", filename.strip_suffix(".md").unwrap_or(filename))
}

/// Text for a package with no renderable changelog entries, from why it bumps.
fn fallback_reason_text(reason: &Option<BumpReason>) -> String {
    match reason {
        Some(reason) => match reason {
            BumpReason::Changeset { changesets } => {
                format!("Associated changesets: `{}`", changesets.join("`, `"))
            }
            BumpReason::Cascade {
                via,
                spec,
                dependency_to,
                ..
            } => {
                format!(
                    "Automatic dependency cascade triggered by `{}` (spec `{}` target `{}`).",
                    via.display_name(),
                    spec,
                    dependency_to.render()
                )
            }
            BumpReason::PeerEscalation { via, spec } => {
                format!(
                    "Peer dependency escalation triggered by `{}` (spec `{}`).",
                    via.display_name(),
                    spec
                )
            }
            BumpReason::FixedGroupUnion { group } => {
                format!("Fixed group synchronization for group `{}`.", group.as_str())
            }
            BumpReason::LinkedGroupUnion { group } => {
                format!("Linked group version alignment for group `{}`.", group.as_str())
            }
            BumpReason::Inference { commits, .. } => {
                format!("Inferred from {commits} commit(s) since the last release.")
            }
            BumpReason::PreRelease { tag } => {
                format!("Pre-release finalization (tag `{tag}`).")
            }
            BumpReason::NewGroupMember { group } => {
                format!("New member of group `{}`.", group.as_str())
            }
            _ => "_No changelog entries available._".to_string(),
        },
        None => "_No changelog entries available._".to_string(),
    }
}

/// More changesets than this are counted in the Reason column rather than named.
const MAX_NAMED_CHANGESETS: usize = 3;

/// Changesets retained from earlier in a pre cycle that set a bump above what its trigger asks for.
/// Defined here so only the Reason column can read them: they are never changelog entries.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct SeveritySources(Vec<String>);

impl SeveritySources {
    pub(crate) fn new(changesets: Vec<String>) -> Self {
        Self(changesets)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The changesets as code spans, or `None` when there are too many to name.
fn changeset_names(changesets: &[String]) -> Option<String> {
    (changesets.len() <= MAX_NAMED_CHANGESETS).then(|| {
        let names: Vec<String> = changesets.iter().map(|name| name.replace('|', "\\|")).collect();
        format!("`{}`", names.join("`, `"))
    })
}

/// The package's reason, then the retained changesets that set its bump above what that reason asks for.
fn package_reason_cell(package: &PackageRelease) -> String {
    let reason = reason_cell(&package.reason);
    let sources = &package.severity_sources.0;
    if sources.is_empty() {
        return reason;
    }
    let names = changeset_names(sources).unwrap_or_else(|| format!("{} changesets", sources.len()));
    format!("{reason}; {} from {names}", package.severity)
}

fn reason_cell(reason: &Option<BumpReason>) -> String {
    match reason {
        Some(BumpReason::Changeset { changesets }) => match changeset_names(changesets) {
            Some(names) => format!("Changeset ({names})"),
            None => format!("{} changesets", changesets.len()),
        },
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
    let preview = crate::release_preview::build_release_preview_in(ws, &plan)?;
    Ok(render_pr_body_from_preview(&preview, opts))
}

/// The PR body for a preview, e.g. one written earlier by `version --emit-preview`.
pub fn render_pr_body_from_preview(preview: &ReleasePreview, opts: &PrBodyOptions) -> ComposePrBodyReport {
    ComposePrBodyReport {
        schema_version: SCHEMA_VERSION,
        body: render_pr_body(preview, opts),
        labels: preview.pr_labels(),
        diagnostics: Vec::new(),
    }
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
        body.push_str("> **No pending changesets found.** No package is queued for versioning.\n\n");
        body.push_str(&attention(&notes.findings));
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
            package_reason_cell(package)
        ));
    }
    body.push('\n');

    body.push_str(&callout(&notes.findings, notes.packages.len()));
    body.push_str(&attention(&notes.findings));
    if let Some(PublishOutlook::Planned {
        releases,
        notes,
        versioned_only,
    }) = &notes.publish
    {
        body.push_str(&publish_section(releases, notes, versioned_only));
    }

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
        render_level(&mut body, level, &in_level);
    }

    body.push_str(&workflow_instructions(branch));
    body
}

/// Changes are grouped by their highest bump; a bump of `none` reads as a patch, and a dependency update is always a
/// patch, as in the changelog.
fn change_level(change: &Change) -> Severity {
    if matches!(change.kind, ChangeKind::DependencyUpdate { .. }) {
        return Severity::Patch;
    }
    match change.max_severity() {
        Severity::None => Severity::Patch,
        level => level,
    }
}

fn level_title(level: Severity) -> &'static str {
    match level {
        Severity::Major => "Major",
        Severity::Minor => "Minor",
        Severity::Patch | Severity::None => "Patch",
    }
}

/// One group's collapsible block: a summary written here, then each change as the changelog will render it.
fn render_level(body: &mut String, level: Severity, changes: &[&Change]) {
    let open = if level == Severity::Major { " open" } else { "" };
    let noun = if changes.len() == 1 { "change" } else { "changes" };
    body.push_str(&format!(
        "<details{open}>\n<summary>{}: {} {noun}</summary>\n\n",
        level_title(level),
        changes.len()
    ));
    for bullet in changes.iter().filter_map(|change| change_bullet(change)) {
        body.push_str(&bullet);
    }
    body.push_str(&dependency_updates(changes));
    body.push_str("</details>\n\n");
}

/// The changelog's one nested "Dependency updates" block for the group, each update naming its own packages.
fn dependency_updates(changes: &[&Change]) -> String {
    let mut out = String::new();
    for change in changes {
        if let ChangeKind::DependencyUpdate { dependency, to } = &change.kind {
            if out.is_empty() {
                out.push_str(DEPENDENCY_HEADING);
            }
            let names: Vec<String> = change.members.iter().map(|m| format!("`{}`", m.package)).collect();
            out.push_str(&format!(
                "{} · affects {}\n",
                dependency_bullet(dependency, to),
                names.join(", ")
            ));
        }
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// The changelog's bullet for the change, then who it affects and the pull request that added it.
/// Dependency updates have no bullet: they render as one block per group.
fn change_bullet(change: &Change) -> Option<String> {
    let mut bullet = match &change.kind {
        ChangeKind::Changeset { summary, filename, .. } => match indent_changeset_summary(summary) {
            Some(indented) => format!("- {indented}\n"),
            None => format!("- {}\n", changeset_label(filename)),
        },
        ChangeKind::Fallback { reason } => {
            format!("- `{}`: {}\n", change.members[0].package, fallback_reason_text(reason))
        }
        ChangeKind::Commit { subject, short_sha, .. } => commit_bullet(subject, short_sha),
        ChangeKind::PeerEscalation { dependency, to } => peer_bullet(dependency, to),
        ChangeKind::DependencyUpdate { .. } => return None,
    };
    if !matches!(change.kind, ChangeKind::Fallback { .. }) {
        let affected: Vec<String> = change
            .members
            .iter()
            .map(|member| format!("`{}`", member.package))
            .collect();
        let pr = match &change.kind {
            ChangeKind::Changeset { pr: Some(pr), .. } => format!(" · #{pr}"),
            _ => String::new(),
        };
        bullet.push_str(&format!("\n  Affects {}{pr}\n", affected.join(", ")));
    }
    bullet.push('\n');
    Some(bullet)
}

/// Where merging will publish each package, from planning the versioned tree's release.
fn publish_section(releases: &[PublishTargets], notes: &[String], versioned_only: &[String]) -> String {
    let mut out = String::new();
    if !releases.is_empty() {
        out.push_str("### 🚀 Merging will publish\n\n| Package | Goes to |\n| :--- | :--- |\n");
    }
    for release in releases {
        let mut targets = release.registries.clone();
        if release.tag {
            targets.push("git tag".to_owned());
        }
        if release.github_release {
            targets.push("GitHub release".to_owned());
        }
        if release.artifacts > 0 {
            let noun = if release.artifacts == 1 { "asset" } else { "assets" };
            targets.push(format!("{} {noun}", release.artifacts));
        }
        out.push_str(&format!(
            "| `{}` `{}` | {} |\n",
            release.package,
            release.version,
            targets.join(" · ")
        ));
    }
    if !releases.is_empty() {
        out.push('\n');
    }
    if !versioned_only.is_empty() {
        let names: Vec<String> = versioned_only.iter().map(|name| format!("`{name}`")).collect();
        out.push_str(&format!(
            "Versioned, not published: {} (no publish target).\n\n",
            names.join(", ")
        ));
    }
    if !releases.is_empty() {
        for note in notes {
            out.push_str(&format!("_{note}_\n\n"));
        }
    }
    out
}

/// The callout that opens the release PR body.
fn callout(findings: &[Finding], package_count: usize) -> String {
    if let Some(stop) = findings.iter().find(|finding| finding.level == FindingLevel::Stop) {
        return format!(
            "> [!CAUTION]\n> **This release would fail**: {}\n> The release workflow runs the same step and would stop with this error after merge. Fix it before merging.\n\n",
            stop.message
        );
    }
    if let Some(major) = findings.iter().find(|finding| finding.id == FINDING_MAJOR_BUMP) {
        return format!(
            "> [!WARNING]\n> **Major Version Bumps Detected**: {}\n\n",
            major.message
        );
    }
    let verdict = findings
        .iter()
        .find(|finding| matches!(finding.id.as_str(), "pre-1.0-minor" | "compatible"))
        .map_or("Read the items below before merging.", |finding| {
            finding.message.as_str()
        });
    format!("> [!NOTE]\n> **{package_count} package(s)** queued for versioning. {verdict}\n\n")
}

/// What a reader of the release should see before merging: breaking and action markers from the changesets, and
/// packages whose changes this release leaves out.
fn attention(findings: &[Finding]) -> String {
    let items: String = findings
        .iter()
        .filter_map(|finding| {
            let label = match finding.id.as_str() {
                FINDING_BREAKING_MARKER => "Breaking",
                FINDING_ACTION_REQUIRED => "Action required",
                FINDING_BREAKING_BUMP_MISMATCH => "Check the bump",
                FINDING_UNRELEASED_CHANGES => "Changed, not releasing",
                _ => return None,
            };
            let who: Vec<String> = finding.packages.iter().map(|package| format!("`{package}`")).collect();
            Some(format!("> - **{label}** ({}): {}\n", who.join(", "), finding.message))
        })
        .collect();
    if items.is_empty() {
        return String::new();
    }
    format!("> [!IMPORTANT]\n> **Read before merging**\n{items}\n")
}

#[cfg(test)]
fn review_callout(bumps: &[crate::plan::PlannedBump]) -> String {
    callout(&crate::release_preview::assess(bumps), bumps.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{PlannedBump, VersionPlan};
    use crate::release_preview::build_release_preview;
    use callisto_model::{BumpReason, PackageId, Severity, Version};

    fn preview_of_one_bump() -> ReleasePreview {
        let plan = VersionPlan {
            bumps: vec![bump(
                Version::semver(1, 0, 0),
                Version::semver(1, 1, 0),
                Severity::Minor,
            )],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![],
            consumed_changesets: vec![],
            archived_changesets: Vec::new(),
            inferred: Default::default(),
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };
        build_release_preview(&plan)
    }

    #[test]
    fn an_empty_plan_still_says_which_packages_changed_without_a_release() {
        use crate::release_preview::UnreleasedChange;
        let plan = VersionPlan {
            bumps: vec![],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![],
            consumed_changesets: vec![],
            archived_changesets: Vec::new(),
            inferred: Default::default(),
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };
        let preview = build_release_preview(&plan).with_unreleased(vec![UnreleasedChange {
            package: "other".to_owned(),
            files: 2,
        }]);

        let body = render_pr_body_from_preview(&preview, &PrBodyOptions::default()).body;

        assert!(body.contains("**No pending changesets found.**"), "{body}");
        assert!(
            !body.contains("up to date"),
            "it must not claim there is nothing to release: {body}"
        );
        assert!(body.contains("> - **Changed, not releasing** (`other`)"), "{body}");
    }

    #[test]
    fn markers_and_unreleased_packages_lead_the_body_in_one_callout() {
        use crate::release_preview::UnreleasedChange;
        let mut preview = preview_of_one_bump().with_unreleased(vec![UnreleasedChange {
            package: "other".to_owned(),
            files: 2,
        }]);
        preview.findings.push(Finding {
            id: FINDING_BREAKING_MARKER.to_owned(),
            level: FindingLevel::Pause,
            message: "`--old` is gone.".to_owned(),
            packages: vec!["core-crate".to_owned(), "cli".to_owned()],
        });
        preview.findings.retain(|finding| finding.id != "compatible");

        let body = render_pr_body_from_preview(&preview, &PrBodyOptions::default()).body;

        assert!(
            body.contains(
                "> [!IMPORTANT]\n> **Read before merging**\n> - **Changed, not releasing** (`other`): Source changed"
            ),
            "{body}"
        );
        assert!(
            body.contains("> - **Breaking** (`core-crate`, `cli`): `--old` is gone.\n"),
            "{body}"
        );
        assert!(
            !body.contains("backward compatible"),
            "a breaking marker removes the claim: {body}"
        );
        assert!(body.contains("Read the items below before merging."), "{body}");
    }

    #[test]
    fn a_bump_that_contradicts_a_breaking_marker_is_listed_for_the_reader() {
        let mut preview = preview_of_one_bump();
        preview.findings.push(Finding {
            id: FINDING_BREAKING_BUMP_MISMATCH.to_owned(),
            level: FindingLevel::Pause,
            message: "Marked Breaking, but this change bumps only `patch`; a breaking change needs `major`.".to_owned(),
            packages: vec!["core-crate".to_owned()],
        });

        let body = render_pr_body_from_preview(&preview, &PrBodyOptions::default()).body;

        assert!(
            body.contains("> - **Check the bump** (`core-crate`): Marked Breaking, but this change bumps only `patch`"),
            "{body}"
        );
    }

    #[test]
    fn a_body_with_no_markers_or_unreleased_packages_has_no_attention_block() {
        let body = render_pr_body_from_preview(&preview_of_one_bump(), &PrBodyOptions::default()).body;
        assert!(!body.contains("Read before merging"), "{body}");
    }

    #[test]
    fn a_planned_release_lists_where_each_package_goes() {
        use crate::release_preview::{PublishOutlook, PublishTargets};
        let preview = preview_of_one_bump().with_publish(PublishOutlook::Planned {
            releases: vec![PublishTargets {
                package: "core-crate".to_owned(),
                version: "1.1.0".to_owned(),
                registries: vec!["crates.io".to_owned()],
                tag: true,
                github_release: true,
                artifacts: 5,
            }],
            notes: vec!["`origin` has no push URL.".to_owned()],
            versioned_only: Vec::new(),
        });

        let body = render_pr_body_from_preview(&preview, &PrBodyOptions::default()).body;
        assert!(!body.contains("Versioned, not published"), "{body}");

        assert!(
            body.contains("### 🚀 Merging will publish\n\n| Package | Goes to |\n| :--- | :--- |\n| `core-crate` `1.1.0` | crates.io · git tag · GitHub release · 5 assets |\n"),
            "{body}"
        );
        assert!(body.contains("_`origin` has no push URL._"), "{body}");
        assert!(!body.contains("[!CAUTION]"), "{body}");
    }

    #[test]
    fn versioned_only_packages_get_one_line_after_the_publish_table() {
        use crate::release_preview::{PublishOutlook, PublishTargets};
        let planned = |releases, versioned_only| {
            let preview = preview_of_one_bump().with_publish(PublishOutlook::Planned {
                releases,
                notes: Vec::new(),
                versioned_only,
            });
            render_pr_body_from_preview(&preview, &PrBodyOptions::default()).body
        };
        let release = PublishTargets {
            package: "core-crate".to_owned(),
            version: "1.1.0".to_owned(),
            registries: vec!["crates.io".to_owned()],
            tag: true,
            github_release: false,
            artifacts: 0,
        };

        let body = planned(
            vec![release],
            vec!["callisto-fixtures".to_owned(), "other-private".to_owned()],
        );
        assert!(
            body.contains(
                "| `core-crate` `1.1.0` | crates.io · git tag |\n\nVersioned, not published: `callisto-fixtures`, `other-private` (no publish target).\n\n"
            ),
            "{body}"
        );

        let body = planned(Vec::new(), vec!["callisto-fixtures".to_owned()]);
        assert!(
            body.contains("Versioned, not published: `callisto-fixtures` (no publish target).\n"),
            "{body}"
        );
        assert!(!body.contains("Merging will publish"), "{body}");

        let body = planned(Vec::new(), Vec::new());
        assert!(!body.contains("Versioned, not published"), "{body}");
    }

    #[test]
    fn a_release_that_would_fail_leads_with_a_caution_and_lists_nothing() {
        use crate::release_preview::PublishOutlook;
        let preview = preview_of_one_bump().with_publish(PublishOutlook::Blocked {
            code: Some("E320".to_owned()),
            message: "version 1.1.0 is already on the registry".to_owned(),
        });

        let body = render_pr_body_from_preview(&preview, &PrBodyOptions::default()).body;

        assert!(
            body.contains(
                "> [!CAUTION]\n> **This release would fail**: E320: version 1.1.0 is already on the registry\n"
            ),
            "{body}"
        );
        assert!(!body.contains("Merging will publish"), "{body}");
        assert!(
            !body.contains("[!NOTE]"),
            "the caution replaces the compatibility note: {body}"
        );
    }

    #[test]
    fn a_preview_without_a_publish_outlook_renders_no_publish_section() {
        let body = render_pr_body_from_preview(&preview_of_one_bump(), &PrBodyOptions::default()).body;
        assert!(!body.contains("Merging will publish"), "{body}");
    }

    #[test]
    fn a_change_names_the_pull_request_that_added_it() {
        let write = |package: &str, filename: &str, summary: &str| {
            let package = PackageId::parse(package).unwrap();
            let bump = PlannedBump {
                package: package.clone(),
                from: Version::semver(1, 0, 0),
                to: Version::semver(1, 1, 0),
                severity: Severity::Minor,
                governed_by: None,
                trigger: None,
                writes: vec![],
                severity_sources: Default::default(),
            };
            let write = crate::plan::ChangelogWrite {
                changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
                input: crate::changelog::ChangelogInput {
                    style: Default::default(),
                    package,
                    from: Version::semver(1, 0, 0),
                    to: Some(Version::semver(1, 1, 0)),
                    entries: vec![crate::changelog::ChangelogEntry {
                        severity: Severity::Minor,
                        source: crate::changelog::ChangeSource::Changeset {
                            filename: filename.to_owned(),
                            summary: summary.to_owned(),
                        },
                    }],
                },
            };
            (bump, write)
        };
        let (a_bump, a_write) = write("pkg-a", "a", "Adds a flag.");
        let (b_bump, b_write) = write("pkg-b", "b", "Drops it.\n\nDetails here.");
        let (c_bump, c_write) = write("pkg-c", "c", "No pull request.");
        let plan = VersionPlan {
            bumps: vec![a_bump, b_bump, c_bump],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![a_write, b_write, c_write],
            consumed_changesets: vec![],
            archived_changesets: Vec::new(),
            inferred: Default::default(),
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };
        let prs = std::collections::BTreeMap::from([("a".to_owned(), 12), ("b".to_owned(), 34)]);
        let preview = build_release_preview(&plan).with_change_prs(&prs);

        let body = render_pr_body_from_preview(&preview, &PrBodyOptions::default()).body;

        assert!(body.contains("- Adds a flag.\n\n  Affects `pkg-a` · #12\n"), "{body}");
        assert!(body.contains("  Details here.\n\n  Affects `pkg-b` · #34\n"), "{body}");
        assert!(body.contains("- No pull request.\n\n  Affects `pkg-c`\n"), "{body}");
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
                trigger: None,
                writes: vec![],
                severity_sources: Default::default(),
            };
            let write = crate::plan::ChangelogWrite {
                changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
                input: crate::changelog::ChangelogInput {
                    style: Default::default(),
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
            "Drops the old API.\n\n## Migration\n\nCallers must now pass a config.",
        );
        let plan = VersionPlan {
            bumps: vec![minor_bump, major_bump],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![minor_write, major_write],
            consumed_changesets: vec![],
            archived_changesets: Vec::new(),
            inferred: Default::default(),
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };

        let body = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default()).body;

        let major = body.find("#### 🔴 Major").expect("major heading");
        let minor = body.find("#### 🟡 Minor").expect("minor heading");
        assert!(major < minor, "{body}");
        assert!(!body.contains("#### 🟢 Patch"), "{body}");
        assert!(
            body.contains("<details open>\n<summary>Major: 1 change</summary>\n\n- Drops the old API."),
            "{body}"
        );
        assert!(body.contains("Affects `pkg-b`\n"), "{body}");
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
            trigger: None,
            writes: vec![],
            severity_sources: Default::default(),
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
            trigger: Some(BumpReason::Changeset {
                changesets: vec!["swift-foxes-race".to_string()],
            }),
            writes: vec![crate::plan::VersionWriteTarget::Manifest(
                "core-crate/Cargo.toml".into(),
            )],
            severity_sources: Default::default(),
        };

        let bump_b = PlannedBump {
            package: pkg_b,
            from: Version::semver(1, 0, 0),
            to: Version::semver(1, 0, 1),
            severity: Severity::Patch,
            governed_by: None,
            trigger: Some(BumpReason::Changeset {
                changesets: vec!["swift-foxes-race".to_string()],
            }),
            writes: vec![crate::plan::VersionWriteTarget::Manifest("web-app/package.json".into())],
            severity_sources: Default::default(),
        };

        let plan = VersionPlan {
            bumps: vec![bump_a, bump_b],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![],
            consumed_changesets: vec![std::path::PathBuf::from(".changeset/swift-foxes-race.md")],
            archived_changesets: Vec::new(),
            inferred: Default::default(),
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };

        let opts = PrBodyOptions::default();
        let report = render_pr_body_from_preview(&build_release_preview(&plan), &opts);
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
            archived_changesets: Vec::new(),
            inferred: Default::default(),
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
        let report = render_pr_body_from_preview(&build_release_preview(&plan), &opts);
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
                trigger: Some(reason),
                writes: vec![],
                severity_sources: Default::default(),
            }],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![],
            consumed_changesets: vec![],
            archived_changesets: Vec::new(),
            inferred: Default::default(),
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
        let report = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default());
        assert_release_reason_has_content(&report.body);
    }

    #[test]
    fn prerelease_fallback_is_non_empty() {
        let plan = plan_with_single_bump(BumpReason::PreRelease {
            tag: "alpha".to_string(),
        });
        let report = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default());
        assert_release_reason_has_content(&report.body);
    }

    #[test]
    fn new_group_member_fallback_is_non_empty() {
        let plan = plan_with_single_bump(BumpReason::NewGroupMember {
            group: callisto_model::GroupName("grp".to_string()),
        });
        let report = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default());
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
            trigger: Some(BumpReason::Cascade {
                via: PackageId::parse("pkg-core").unwrap(),
                dep_kind: callisto_model::DepKind::Runtime,
                spec: "^1.0.0".to_string(),
                dependency_to: Version::semver(2, 0, 0),
            }),
            writes: vec![],
            severity_sources: Default::default(),
        };

        let input = crate::changelog::ChangelogInput {
            style: Default::default(),
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
        // Checks `render_section` succeeds without asserting its heading, which is meaningless once entries are regrouped.
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
            archived_changesets: Vec::new(),
            inferred: Default::default(),
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };

        let report = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default());

        assert!(
            report.body.contains("A real changeset summary."),
            "body must contain the real changeset entry's text; body: {}",
            report.body
        );
        assert!(
            report.body.contains("`pkg-a`"),
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
    fn a_changeset_name_with_a_pipe_is_escaped_in_the_reason_cell() {
        let cell = reason_cell(&Some(BumpReason::Changeset {
            changesets: vec!["a|b".to_string()],
        }));
        assert_eq!(cell, "Changeset (`a\\|b`)");
    }

    /// More retained sources than can be named are counted after the trigger.
    #[test]
    fn many_severity_sources_are_counted_in_the_reason_cell() {
        let package = |sources: &[&str]| PackageRelease {
            name: "pkg-a".into(),
            release_id: None,
            ecosystem: "cargo".into(),
            from: "2.0.0-beta.0".into(),
            to: "2.0.0-beta.1".into(),
            severity: Severity::Major,
            reason: Some(BumpReason::Changeset {
                changesets: vec!["fix".to_string()],
            }),
            severity_sources: SeveritySources::new(sources.iter().map(|id| id.to_string()).collect()),
        };
        assert_eq!(
            package_reason_cell(&package(&["a", "b", "c"])),
            "Changeset (`fix`); major from `a`, `b`, `c`"
        );
        assert_eq!(
            package_reason_cell(&package(&["a", "b", "c", "d"])),
            "Changeset (`fix`); major from 4 changesets"
        );
    }

    #[test]
    fn changes_render_as_the_changelog_will_write_them() {
        let pkg = PackageId::parse("pkg-a").unwrap();
        let entry = |filename: &str, summary: &str| crate::changelog::ChangelogEntry {
            severity: Severity::Patch,
            source: crate::changelog::ChangeSource::Changeset {
                filename: filename.to_string(),
                summary: summary.to_string(),
            },
        };
        let input = crate::changelog::ChangelogInput {
            style: Default::default(),
            package: pkg.clone(),
            from: Version::semver(1, 0, 0),
            to: Some(Version::semver(1, 0, 1)),
            entries: vec![
                entry(
                    "brave-eagles-race.md",
                    "**Python ranges keep their precision when a bound has a suffix.** A dependent requirement whose bound carries a post, dev or pre-release suffix is now rewritten at the precision it was written with, in any epoch.\n\n- `>=1.2.3.post1,<1.3` becomes `>=1.2.4,<1.3` instead of widening to `<2`.\n- `~=1.2.post1` becomes `~=1.2` instead of narrowing to `~=1.2.1`.",
                ),
                entry(
                    "radiant-pandas-roar.md",
                    "**`pre enter` and `pre exit` stage only their own `pre.json`.** A changesets directory whose name contains glob characters, such as `notes*`, no longer stages a neighbouring tracked `notes-other/pre.json` as well.",
                ),
            ],
        };
        let plan = VersionPlan {
            bumps: vec![
                PlannedBump {
                    package: pkg,
                    from: Version::semver(1, 0, 0),
                    to: Version::semver(1, 0, 1),
                    severity: Severity::Patch,
                    governed_by: None,
                    trigger: Some(BumpReason::Changeset {
                        changesets: vec!["brave-eagles-race".to_string(), "radiant-pandas-roar".to_string()],
                    }),
                    writes: vec![],
                    severity_sources: Default::default(),
                },
                PlannedBump {
                    package: PackageId::parse("pkg-b").unwrap(),
                    from: Version::semver(1, 0, 0),
                    to: Version::semver(1, 0, 1),
                    severity: Severity::Patch,
                    governed_by: None,
                    trigger: Some(BumpReason::Changeset {
                        changesets: ["a", "b", "c", "d"].map(String::from).to_vec(),
                    }),
                    writes: vec![],
                    severity_sources: Default::default(),
                },
            ],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![crate::plan::ChangelogWrite {
                changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
                input,
            }],
            consumed_changesets: vec![],
            archived_changesets: Vec::new(),
            inferred: Default::default(),
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };

        let body = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default()).body;

        insta::assert_snapshot!(body);
    }

    #[test]
    fn dependency_updates_nest_under_patch_as_the_changelog_writes_them() {
        let pkg = PackageId::parse("pkg-a").unwrap();
        let input = crate::changelog::ChangelogInput {
            style: Default::default(),
            package: pkg.clone(),
            from: Version::semver(1, 0, 0),
            to: Some(Version::semver(1, 1, 0)),
            entries: vec![
                crate::changelog::ChangelogEntry {
                    severity: Severity::Minor,
                    source: crate::changelog::ChangeSource::Changeset {
                        filename: "adds-flag.md".to_string(),
                        summary: "**Adds a flag.** Use it.".to_string(),
                    },
                },
                crate::changelog::ChangelogEntry {
                    severity: Severity::Minor,
                    source: crate::changelog::ChangeSource::DependencyUpdate {
                        dependency: PackageId::parse("pkg-core").unwrap(),
                        dep_kind: callisto_model::DepKind::Runtime,
                        to: Version::semver(2, 0, 0),
                    },
                },
            ],
        };
        let plan = VersionPlan {
            bumps: vec![PlannedBump {
                package: pkg,
                from: Version::semver(1, 0, 0),
                to: Version::semver(1, 1, 0),
                severity: Severity::Minor,
                governed_by: None,
                trigger: None,
                writes: vec![],
                severity_sources: Default::default(),
            }],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![crate::plan::ChangelogWrite {
                changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
                input,
            }],
            consumed_changesets: vec![],
            archived_changesets: Vec::new(),
            inferred: Default::default(),
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };

        let body = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default()).body;

        insta::assert_snapshot!(body);
    }

    #[test]
    fn a_newline_in_a_dependency_name_keeps_each_update_with_its_own_packages() {
        let dep_bump = |name: &str, dep: &str| {
            let pkg = PackageId::parse(name).unwrap();
            let input = crate::changelog::ChangelogInput {
                style: Default::default(),
                package: pkg.clone(),
                from: Version::semver(1, 0, 0),
                to: Some(Version::semver(1, 0, 1)),
                entries: vec![crate::changelog::ChangelogEntry {
                    severity: Severity::Patch,
                    source: crate::changelog::ChangeSource::DependencyUpdate {
                        dependency: PackageId::parse(dep).unwrap(),
                        dep_kind: callisto_model::DepKind::Runtime,
                        to: Version::semver(2, 0, 0),
                    },
                }],
            };
            let bump = PlannedBump {
                package: pkg,
                from: Version::semver(1, 0, 0),
                to: Version::semver(1, 0, 1),
                severity: Severity::Patch,
                governed_by: None,
                trigger: None,
                writes: vec![],
                severity_sources: Default::default(),
            };
            (bump, input)
        };
        let (bump_a, input_a) = dep_bump("app-a", "dep\nx");
        let (bump_b, input_b) = dep_bump("app-b", "dep-y");
        let plan = VersionPlan {
            bumps: vec![bump_a, bump_b],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: [("A", input_a), ("B", input_b)]
                .into_iter()
                .map(|(n, input)| crate::plan::ChangelogWrite {
                    changelog_path: std::path::PathBuf::from(format!("{n}/CHANGELOG.md")),
                    input,
                })
                .collect(),
            consumed_changesets: vec![],
            archived_changesets: Vec::new(),
            inferred: Default::default(),
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };

        let body = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default()).body;

        assert!(body.contains("`dep-y` → `2.0.0` · affects `app-b`"), "{body}");
        assert!(body.contains("→ `2.0.0` · affects `app-a`"), "{body}");
        assert!(!body.contains("`dep-y` → `2.0.0` · affects `app-a`"), "{body}");
    }

    #[test]
    fn each_dependency_update_names_only_its_own_packages() {
        let dep_bump = |name: &str, dep: &str, to: u64| {
            let pkg = PackageId::parse(name).unwrap();
            let input = crate::changelog::ChangelogInput {
                style: Default::default(),
                package: pkg.clone(),
                from: Version::semver(1, 0, 0),
                to: Some(Version::semver(1, 0, 1)),
                entries: vec![crate::changelog::ChangelogEntry {
                    severity: Severity::Patch,
                    source: crate::changelog::ChangeSource::DependencyUpdate {
                        dependency: PackageId::parse(dep).unwrap(),
                        dep_kind: callisto_model::DepKind::Runtime,
                        to: Version::semver(to, 0, 0),
                    },
                }],
            };
            let bump = PlannedBump {
                package: pkg,
                from: Version::semver(1, 0, 0),
                to: Version::semver(1, 0, 1),
                severity: Severity::Patch,
                governed_by: None,
                trigger: None,
                writes: vec![],
                severity_sources: Default::default(),
            };
            (bump, input)
        };
        let (bump_a, input_a) = dep_bump("app-a", "dep-x", 2);
        let (bump_b, input_b) = dep_bump("app-b", "dep-y", 3);
        let plan = VersionPlan {
            bumps: vec![bump_a, bump_b],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: [("A", input_a), ("B", input_b)]
                .into_iter()
                .map(|(n, input)| crate::plan::ChangelogWrite {
                    changelog_path: std::path::PathBuf::from(format!("{n}/CHANGELOG.md")),
                    input,
                })
                .collect(),
            consumed_changesets: vec![],
            archived_changesets: Vec::new(),
            inferred: Default::default(),
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };

        let body = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default()).body;

        insta::assert_snapshot!(body);
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
            trigger: Some(BumpReason::Cascade {
                via: PackageId::parse("pkg-core").unwrap(),
                dep_kind: callisto_model::DepKind::Runtime,
                spec: "^1.0.0".to_string(),
                dependency_to: Version::semver(2, 0, 0),
            }),
            writes: vec![],
            severity_sources: Default::default(),
        };
        let bump_b = PlannedBump {
            package: pkg_b.clone(),
            from: Version::semver(1, 0, 0),
            to: Version::semver(1, 0, 1),
            severity: Severity::Patch,
            governed_by: None,
            trigger: Some(BumpReason::Changeset {
                changesets: vec!["fix-1".to_string()],
            }),
            writes: vec![],
            severity_sources: Default::default(),
        };

        let good_input = crate::changelog::ChangelogInput {
            style: Default::default(),
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
                        style: Default::default(),
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
            archived_changesets: Vec::new(),
            inferred: Default::default(),
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };

        let report = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default());
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
            report.body.contains("`pkg-b`"),
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
            trigger: Some(BumpReason::Cascade {
                via: PackageId::parse("pkg-core").unwrap(),
                dep_kind: callisto_model::DepKind::Runtime,
                spec: "^1.0.0".to_string(),
                dependency_to: Version::semver(2, 0, 0),
            }),
            writes: vec![],
            severity_sources: Default::default(),
        };
        let plan = VersionPlan {
            bumps: vec![bump_a],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![crate::plan::ChangelogWrite {
                changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
                input: crate::changelog::ChangelogInput {
                    style: Default::default(),
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
            archived_changesets: Vec::new(),
            inferred: Default::default(),
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };

        let report = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default());
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
            trigger: None,
            writes: vec![],
            severity_sources: Default::default(),
        };
        let plan = VersionPlan {
            bumps: vec![bump_a],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![crate::plan::ChangelogWrite {
                changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
                input: crate::changelog::ChangelogInput {
                    style: Default::default(),
                    package: pkg_a.clone(),
                    from: Version::semver(1, 0, 0),
                    to: Some(Version::semver(1, 1, 0)),
                    entries: vec![],
                },
            }],
            consumed_changesets: vec![],
            archived_changesets: Vec::new(),
            inferred: Default::default(),
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };

        let report = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default());
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
                style: Default::default(),
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
                    trigger: Some(BumpReason::Changeset {
                        changesets: vec!["delete-dead-abstractions".to_string()],
                    }),
                    writes: vec![],
                    severity_sources: Default::default(),
                },
                PlannedBump {
                    package: pkg_b.clone(),
                    from: Version::semver(1, 0, 0),
                    to: Version::semver(1, 1, 0),
                    severity: Severity::Minor,
                    governed_by: None,
                    trigger: Some(BumpReason::Changeset {
                        changesets: vec!["delete-dead-abstractions".to_string()],
                    }),
                    writes: vec![],
                    severity_sources: Default::default(),
                },
            ],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![make_write(pkg_a), make_write(pkg_b)],
            consumed_changesets: vec![],
            archived_changesets: Vec::new(),
            inferred: Default::default(),
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };

        let report = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default());

        assert_eq!(
            report
                .body
                .matches("Delete unused abstractions with zero real callers")
                .count(),
            1,
            "the shared changeset renders once, not once per package; body: {}",
            report.body
        );
        assert!(
            report.body.contains("Affects `pkg-a`, `pkg-b`"),
            "body: {}",
            report.body
        );
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
        let report = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default());
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
        let minor_report = render_pr_body_from_preview(&build_release_preview(&minor_only), &PrBodyOptions::default());
        assert!(
            minor_report
                .body
                .contains("<details>\n<summary>Minor: 1 change</summary>"),
            "a minor-only change's block must default closed; body: {}",
            minor_report.body
        );

        let mut major_plan = plan_with_single_bump(BumpReason::Changeset {
            changesets: vec!["y".to_string()],
        });
        major_plan.bumps[0].severity = Severity::Major;
        let major_report = render_pr_body_from_preview(&build_release_preview(&major_plan), &PrBodyOptions::default());
        assert!(
            major_report
                .body
                .contains("<details open>\n<summary>Major: 1 change</summary>"),
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
            trigger: Some(BumpReason::FixedGroupUnion {
                group: callisto_model::GroupName("workspace".to_string()),
            }),
            writes: vec![],
            severity_sources: Default::default(),
        };
        let plan = VersionPlan {
            bumps: vec![bump_a],
            rewrites: vec![],
            platform_writes: vec![],
            optional_dep_updates: vec![],
            changelog_writes: vec![crate::plan::ChangelogWrite {
                changelog_path: std::path::PathBuf::from("CHANGELOG.md"),
                input: crate::changelog::ChangelogInput {
                    style: Default::default(),
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
            archived_changesets: Vec::new(),
            inferred: Default::default(),
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };

        let report = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default());
        assert!(
            report.body.contains("Also bumped with no direct change of their own"),
            "body: {}",
            report.body
        );
        assert!(report.body.contains("`pkg-a`"), "body: {}", report.body);
        assert_eq!(
            report.body.matches("Affects `pkg-a`").count(),
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
        let report = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default());
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
            trigger: Some(BumpReason::Changeset {
                changesets: vec![changeset.to_string()],
            }),
            writes: vec![],
            severity_sources: Default::default(),
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
            archived_changesets: Vec::new(),
            inferred: Default::default(),
            pre_state_update: None,
            pre_json_path: std::path::PathBuf::new(),
            pre_json_original_text: None,
            delete_pre_json: None,
            diagnostics: vec![],
        };

        let report = render_pr_body_from_preview(&build_release_preview(&plan), &PrBodyOptions::default());

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
