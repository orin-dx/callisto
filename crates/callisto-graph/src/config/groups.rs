use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use callisto_model::{GroupKind, GroupName, ManifestRole, PackageId, Severity};
use serde::Deserialize;

use crate::error::{ConfigError, GraphError};
use crate::identity::IdentityIndex;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupTable {
    pub fixed: BTreeMap<GroupName, GroupDef>,
    pub linked: BTreeMap<GroupName, GroupDef>,
    pub fixed_of: BTreeMap<PackageId, GroupName>,
    pub linked_of: BTreeMap<PackageId, GroupName>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupDef {
    pub name: GroupName,
    pub kind: GroupKind,
    pub members: Vec<GroupMember>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GroupMember {
    Package(PackageId),
    PlatformManifest {
        owner: PackageId,
        role: ManifestRole,
        path: PathBuf,
        name: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GroupMemberKind {
    Package,
    PlatformManifest,
}

impl GroupMember {
    pub fn kind(&self) -> GroupMemberKind {
        match self {
            GroupMember::Package(_) => GroupMemberKind::Package,
            GroupMember::PlatformManifest { .. } => GroupMemberKind::PlatformManifest,
        }
    }
}

impl GroupDef {
    pub fn members(&self, kind: GroupMemberKind) -> impl Iterator<Item = &GroupMember> {
        self.members.iter().filter(move |m| m.kind() == kind)
    }

    /// Package members only, as bare identities -- the shape every
    /// severity/target computation over a group actually needs.
    pub fn package_members(&self) -> impl Iterator<Item = &PackageId> {
        self.members(GroupMemberKind::Package).filter_map(|m| match m {
            GroupMember::Package(id) => Some(id),
            GroupMember::PlatformManifest { .. } => None,
        })
    }

    /// Highest severity among this group's package members that have an
    /// entry in `severities`, or `Severity::None` if none do. The single
    /// definition of "max severity across a group" -- previously re-derived
    /// independently in `aggregate::union_fixed`/`union_linked`, in two
    /// inline blocks in `cascade::solve_cascade`, and again inside
    /// `groups::fixed_group_target`. Having four independent copies let one
    /// of them (cascade's Linked-group block) diverge and skip the
    /// stale-member guard the others had, reaching a `MissingField` crash
    /// for a group member removed from the workspace.
    pub fn max_severity(&self, severities: &BTreeMap<PackageId, Severity>) -> Severity {
        self.package_members()
            .filter_map(|id| severities.get(id).copied())
            .max()
            .unwrap_or(Severity::None)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct RawGroupTable {
    pub fixed: Vec<RawGroup>,
    pub linked: Vec<RawGroup>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawGroup {
    pub name: GroupName,
    pub members: Vec<String>,
}

impl GroupTable {
    pub(crate) fn validate_syntactic(raw: &RawGroupTable) -> Result<(), ConfigError> {
        let mut seen_names = BTreeSet::new();
        for group_list in [&raw.fixed, &raw.linked] {
            for g in group_list {
                if !seen_names.insert(&g.name) {
                    return Err(ConfigError::DuplicateGroupName { group: g.name.clone() });
                }
                if g.members.is_empty() {
                    return Err(ConfigError::EmptyGroup { group: g.name.clone() });
                }
            }
        }

        // One map, populated fixed-then-linked (declaration order, not file
        // order): a fixed group's members are checked only against other
        // fixed groups seen so far (linked hasn't been touched yet at that
        // point), while a linked group's members are checked against BOTH
        // linked-so-far AND the now-fully-populated fixed map -- exactly the
        // asymmetric cross-check the two-separate-maps version encoded, but
        // as a natural consequence of processing order instead of a second
        // explicit lookup.
        let mut claimed: BTreeMap<&str, &GroupName> = BTreeMap::new();
        for group_list in [&raw.fixed, &raw.linked] {
            for g in group_list {
                for m in &g.members {
                    if let Some(other) = claimed.insert(m.as_str(), &g.name) {
                        return Err(ConfigError::ConflictingGroupNames {
                            group: g.name.clone(),
                            other: other.clone(),
                            member: m.clone(),
                        });
                    }
                }
            }
        }

        Ok(())
    }

    pub(crate) fn resolve(raw: &RawGroupTable, index: &IdentityIndex) -> Result<GroupTable, GraphError> {
        let mut fixed = BTreeMap::new();
        let mut linked = BTreeMap::new();
        let mut fixed_of = BTreeMap::new();
        let mut linked_of = BTreeMap::new();
        let mut claimed_by: BTreeMap<PackageId, GroupName> = BTreeMap::new();

        // One resolution loop parameterized on `GroupKind`, writing into
        // whichever kind's own output maps -- `claimed_by` is shared across
        // both passes exactly as before, so a package claimed by a fixed
        // group still conflicts if a linked group (or a second fixed group)
        // also claims it.
        for (kind, raw_groups, groups, membership) in [
            (GroupKind::Fixed, &raw.fixed, &mut fixed, &mut fixed_of),
            (GroupKind::Linked, &raw.linked, &mut linked, &mut linked_of),
        ] {
            for rg in raw_groups {
                let mut members = Vec::new();
                for name in &rg.members {
                    if let Ok(id) = index.resolve_human(name, &[]) {
                        if let Some(other) = claimed_by.get(&id) {
                            if other != &rg.name {
                                return Err(GraphError::ConflictingGroupMembership {
                                    package: id.clone(),
                                    groups: vec![other.clone(), rg.name.clone()],
                                });
                            }
                        } else {
                            claimed_by.insert(id.clone(), rg.name.clone());
                        }
                        members.push(GroupMember::Package(id.clone()));
                        membership.insert(id, rg.name.clone());
                    } else if let Some((owner, path, role)) = index.platform.get(name) {
                        members.push(GroupMember::PlatformManifest {
                            owner: owner.clone(),
                            role: role.clone(),
                            path: path.clone(),
                            name: name.clone(),
                        });
                    } else {
                        return Err(GraphError::MissingGroupMember {
                            group: rg.name.clone(),
                            member: name.clone(),
                        });
                    }
                }
                members.sort();
                groups.insert(
                    rg.name.clone(),
                    GroupDef {
                        name: rg.name.clone(),
                        kind,
                        members,
                    },
                );
            }
        }

        Ok(GroupTable {
            fixed,
            linked,
            fixed_of,
            linked_of,
        })
    }

    pub fn fixed_group_of(&self, id: &PackageId) -> Option<&GroupDef> {
        let name = self.fixed_of.get(id)?;
        self.fixed.get(name)
    }

    pub fn linked_group_of(&self, id: &PackageId) -> Option<&GroupDef> {
        let name = self.linked_of.get(id)?;
        self.linked.get(name)
    }

    pub fn fixed_siblings<'a>(&'a self, id: &'a PackageId) -> impl Iterator<Item = &'a PackageId> {
        let mut sibs = Vec::new();
        if let Some(g) = self.fixed_group_of(id) {
            for m in g.members(GroupMemberKind::Package) {
                if let GroupMember::Package(ref pkg_id) = m {
                    if pkg_id != id {
                        sibs.push(pkg_id);
                    }
                }
            }
        }
        sibs.into_iter()
    }

    pub fn from_groups(fixed: Vec<GroupDef>, linked: Vec<GroupDef>) -> Self {
        let mut f_map = BTreeMap::new();
        let mut l_map = BTreeMap::new();
        let mut f_of = BTreeMap::new();
        let mut l_of = BTreeMap::new();

        for g in fixed {
            for m in g.members(GroupMemberKind::Package) {
                if let GroupMember::Package(ref id) = m {
                    f_of.insert(id.clone(), g.name.clone());
                }
            }
            f_map.insert(g.name.clone(), g);
        }

        for g in linked {
            for m in g.members(GroupMemberKind::Package) {
                if let GroupMember::Package(ref id) = m {
                    l_of.insert(id.clone(), g.name.clone());
                }
            }
            l_map.insert(g.name.clone(), g);
        }

        GroupTable {
            fixed: f_map,
            linked: l_map,
            fixed_of: f_of,
            linked_of: l_of,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_group(name: &str, members: &[&str]) -> RawGroup {
        RawGroup {
            name: GroupName(name.to_string()),
            members: members.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn validate_syntactic_accepts_disjoint_fixed_and_linked_groups() {
        let raw = RawGroupTable {
            fixed: vec![raw_group("fixed-a", &["pkg-a", "pkg-b"])],
            linked: vec![raw_group("linked-a", &["pkg-c", "pkg-d"])],
        };
        assert!(GroupTable::validate_syntactic(&raw).is_ok());
    }

    #[test]
    fn validate_syntactic_rejects_duplicate_name_within_fixed() {
        let raw = RawGroupTable {
            fixed: vec![raw_group("dup", &["pkg-a"]), raw_group("dup", &["pkg-b"])],
            linked: vec![],
        };
        let err = GroupTable::validate_syntactic(&raw).unwrap_err();
        assert!(matches!(err, ConfigError::DuplicateGroupName { group } if group.as_str() == "dup"));
    }

    /// A fixed group and a linked group sharing the same name must also be
    /// rejected -- the duplicate-name check spans both kinds via one shared
    /// `seen_names` set, not just within a single kind.
    #[test]
    fn validate_syntactic_rejects_duplicate_name_across_fixed_and_linked() {
        let raw = RawGroupTable {
            fixed: vec![raw_group("dup", &["pkg-a"])],
            linked: vec![raw_group("dup", &["pkg-b"])],
        };
        let err = GroupTable::validate_syntactic(&raw).unwrap_err();
        assert!(matches!(err, ConfigError::DuplicateGroupName { group } if group.as_str() == "dup"));
    }

    #[test]
    fn validate_syntactic_rejects_empty_fixed_group() {
        let raw = RawGroupTable {
            fixed: vec![raw_group("empty", &[])],
            linked: vec![],
        };
        let err = GroupTable::validate_syntactic(&raw).unwrap_err();
        assert!(matches!(err, ConfigError::EmptyGroup { group } if group.as_str() == "empty"));
    }

    #[test]
    fn validate_syntactic_rejects_empty_linked_group() {
        let raw = RawGroupTable {
            fixed: vec![],
            linked: vec![raw_group("empty", &[])],
        };
        let err = GroupTable::validate_syntactic(&raw).unwrap_err();
        assert!(matches!(err, ConfigError::EmptyGroup { group } if group.as_str() == "empty"));
    }

    #[test]
    fn validate_syntactic_rejects_conflicting_member_within_fixed() {
        let raw = RawGroupTable {
            fixed: vec![raw_group("a", &["shared"]), raw_group("b", &["shared"])],
            linked: vec![],
        };
        let err = GroupTable::validate_syntactic(&raw).unwrap_err();
        assert!(matches!(err, ConfigError::ConflictingGroupNames { member, .. } if member == "shared"));
    }

    #[test]
    fn validate_syntactic_rejects_conflicting_member_within_linked() {
        let raw = RawGroupTable {
            fixed: vec![],
            linked: vec![raw_group("a", &["shared"]), raw_group("b", &["shared"])],
        };
        let err = GroupTable::validate_syntactic(&raw).unwrap_err();
        assert!(matches!(err, ConfigError::ConflictingGroupNames { member, .. } if member == "shared"));
    }

    /// The cross-kind check is one-directional: a linked group's member
    /// conflicting with an already-declared fixed group's member must be
    /// rejected (this is the asymmetric behavior `validate_syntactic`'s
    /// single shared `claimed` map -- populated fixed-then-linked --
    /// reproduces from the original two-separate-maps version).
    #[test]
    fn validate_syntactic_rejects_linked_member_already_claimed_by_fixed() {
        let raw = RawGroupTable {
            fixed: vec![raw_group("fixed-a", &["shared"])],
            linked: vec![raw_group("linked-a", &["shared"])],
        };
        let err = GroupTable::validate_syntactic(&raw).unwrap_err();
        match err {
            ConfigError::ConflictingGroupNames { group, other, member } => {
                assert_eq!(group.as_str(), "linked-a");
                assert_eq!(other.as_str(), "fixed-a");
                assert_eq!(member, "shared");
            }
            other => panic!("expected ConflictingGroupNames, got {other:?}"),
        }
    }
}
