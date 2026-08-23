use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use callisto_model::{GroupKind, GroupName, ManifestRole, PackageId};
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

        for g in &raw.fixed {
            if !seen_names.insert(&g.name) {
                return Err(ConfigError::DuplicateGroupName { group: g.name.clone() });
            }
            if g.members.is_empty() {
                return Err(ConfigError::EmptyGroup { group: g.name.clone() });
            }
        }

        for g in &raw.linked {
            if !seen_names.insert(&g.name) {
                return Err(ConfigError::DuplicateGroupName { group: g.name.clone() });
            }
            if g.members.is_empty() {
                return Err(ConfigError::EmptyGroup { group: g.name.clone() });
            }
        }

        let mut fixed_members = BTreeMap::new();
        for g in &raw.fixed {
            for m in &g.members {
                if let Some(other) = fixed_members.insert(m.as_str(), &g.name) {
                    return Err(ConfigError::ConflictingGroupNames {
                        group: g.name.clone(),
                        other: (*other).clone(),
                        member: m.clone(),
                    });
                }
            }
        }

        let mut linked_members = BTreeMap::new();
        for g in &raw.linked {
            for m in &g.members {
                if let Some(other) = linked_members.insert(m.as_str(), &g.name) {
                    return Err(ConfigError::ConflictingGroupNames {
                        group: g.name.clone(),
                        other: (*other).clone(),
                        member: m.clone(),
                    });
                }
                if let Some(other_fixed) = fixed_members.get(m.as_str()) {
                    return Err(ConfigError::ConflictingGroupNames {
                        group: g.name.clone(),
                        other: (*other_fixed).clone(),
                        member: m.clone(),
                    });
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

        for rg in &raw.fixed {
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
                    fixed_of.insert(id, rg.name.clone());
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
            fixed.insert(
                rg.name.clone(),
                GroupDef {
                    name: rg.name.clone(),
                    kind: GroupKind::Fixed,
                    members,
                },
            );
        }

        for rg in &raw.linked {
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
                    linked_of.insert(id, rg.name.clone());
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
            linked.insert(
                rg.name.clone(),
                GroupDef {
                    name: rg.name.clone(),
                    kind: GroupKind::Linked,
                    members,
                },
            );
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
