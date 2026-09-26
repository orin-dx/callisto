use callisto_model::{CommitSha, DepKind, GroupKind, GroupName, PackageId, Severity, Version};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangelogInput {
    pub package: PackageId,
    pub from: Version,
    pub to: Option<Version>,
    pub entries: Vec<ChangelogEntry>,
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
