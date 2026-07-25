use std::collections::BTreeMap;

use crate::config::groups::{GroupDef, GroupMember, GroupMemberKind, GroupTable};
use crate::error::GraphError;
use crate::napi::NapiTargetsIndex;
use crate::resolver::DependencyResolver;
use crate::tags::TagIndex;
use callisto_format::Versioning;
use callisto_model::{Diagnostic, GroupName, PackageId, Severity, Version};

#[derive(Clone, Debug, Default)]
pub struct GroupCheckOutcome {
    pub new_members: BTreeMap<GroupName, Vec<PackageId>>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn pre_mutation_checks<D: DependencyResolver>(
    _graph: &D,
    groups: &GroupTable,
    base: &BTreeMap<PackageId, Version>,
    tags: &TagIndex,
    _napi: &NapiTargetsIndex,
) -> Result<GroupCheckOutcome, GraphError> {
    let mut outcome = GroupCheckOutcome::default();

    for g in groups.fixed.values() {
        let released: Vec<PackageId> = g
            .members(GroupMemberKind::Package)
            .filter_map(|m| match m {
                GroupMember::Package(ref id) => {
                    if tags.last_tag(id).is_some() {
                        Some(id.clone())
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect();

        let fresh: Vec<PackageId> = g
            .members(GroupMemberKind::Package)
            .filter_map(|m| match m {
                GroupMember::Package(ref id) => {
                    if tags.last_tag(id).is_none() {
                        Some(id.clone())
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect();

        let pairs: Vec<(PackageId, Version)> = released
            .iter()
            .filter_map(|id| base.get(id).map(|v| (id.clone(), v.clone())))
            .collect();

        if pairs.len() > 1 {
            let first_v = &pairs[0].1;
            let mut divergent = false;
            for (_id, v) in &pairs[1..] {
                if v.grammar() != first_v.grammar() {
                    return Err(GraphError::GroupGrammarMismatch {
                        group: g.name.clone(),
                        members: pairs,
                    });
                }
                if Version::compare(v, first_v).ok() != Some(std::cmp::Ordering::Equal) {
                    divergent = true;
                }
            }
            if divergent {
                return Err(GraphError::FixedGroupDivergent {
                    group: g.name.clone(),
                    members: pairs,
                });
            }
        }

        if !fresh.is_empty() {
            outcome.new_members.insert(g.name.clone(), fresh);
        }
    }

    Ok(outcome)
}

pub fn fixed_group_target(
    g: &GroupDef,
    base: &BTreeMap<PackageId, Version>,
    severities: &BTreeMap<PackageId, Severity>,
    tags: &TagIndex,
    _pre: Option<&callisto_format::PreState>,
) -> Result<Version, GraphError> {
    let released: Vec<PackageId> = g
        .members(GroupMemberKind::Package)
        .filter_map(|m| match m {
            GroupMember::Package(ref id) => {
                if tags.last_tag(id).is_some() {
                    Some(id.clone())
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    let mut max_sev = Severity::None;
    for m in g.members(GroupMemberKind::Package) {
        if let GroupMember::Package(ref id) = m {
            if let Some(&s) = severities.get(id) {
                if s > max_sev {
                    max_sev = s;
                }
            }
        }
    }

    let aligned_base = if !released.is_empty() {
        base.get(&released[0])
            .cloned()
            .unwrap_or_else(|| Version::semver(1, 0, 0))
    } else {
        Version::semver(0, 0, 0)
    };

    let versioning = callisto_format::SemVerVersioning;

    versioning
        .bump(&aligned_base, max_sev)
        .map_err(GraphError::Bump)
}
