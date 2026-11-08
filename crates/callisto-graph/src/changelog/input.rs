use callisto_model::{CommitSha, DepKind, GroupKind, GroupName, PackageId, Severity, Version};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangelogInput {
    pub package: PackageId,
    pub from: Version,
    pub to: Option<Version>,
    pub entries: Vec<ChangelogEntry>,
    pub style: ChangelogStyle,
}

/// How a rendered section is laid out. `Grouped` is the configured default; `Flat` is the pre-grouping format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangelogLayout {
    Grouped,
    Flat,
}

/// Layout plus the facts the grouped header shows. The date and link are injected so rendering stays pure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangelogStyle {
    pub layout: ChangelogLayout,
    /// `YYYY-MM-DD`.
    pub date: Option<String>,
    pub compare_url: Option<String>,
}

impl Default for ChangelogStyle {
    /// Flat with no header facts: renders exactly what the tool wrote before layouts existed.
    fn default() -> Self {
        Self {
            layout: ChangelogLayout::Flat,
            date: None,
            compare_url: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangelogEntry {
    pub severity: Severity,
    pub source: ChangeSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeSource {
    Changeset {
        filename: String,
        summary: String,
    },
    Commit {
        sha: CommitSha,
        subject: String,
    },
    DependencyUpdate {
        dependency: PackageId,
        dep_kind: DepKind,
        to: Version,
    },
    PeerEscalation {
        dependency: PackageId,
        to: Version,
    },
    GroupUnion {
        group: GroupName,
        kind: GroupKind,
    },
    NewGroupMember {
        group: GroupName,
    },
}
