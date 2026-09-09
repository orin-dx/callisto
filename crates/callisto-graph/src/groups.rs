use std::collections::BTreeMap;
use std::path::Path;

use crate::config::groups::GroupTable;
use crate::error::GraphError;
use crate::napi::{napi_drift, NapiTargetsIndex};
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
    napi: &NapiTargetsIndex,
    root: &Path,
) -> Result<GroupCheckOutcome, GraphError> {
    let mut outcome = GroupCheckOutcome::default();

    for g in groups.fixed.values() {
        let released: Vec<PackageId> = g
            .package_members()
            .filter(|id| tags.last_tag(id).is_some())
            .cloned()
            .collect();

        let fresh: Vec<PackageId> = g
            .package_members()
            .filter(|id| tags.last_tag(id).is_none())
            .cloned()
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

        // napi.targets drift cross-check (§G.8.4).
        if let Some(declared) = napi.declared_for(&g.name) {
            outcome.diagnostics.extend(napi_drift(g, declared, root));
        }
    }

    Ok(outcome)
}

/// Computes a fixed group's shared alignment target.
///
/// `live_members` must already be filtered to package ids present in
/// `base` (see `solve_cascade`'s Track-1 block) -- a stale group member
/// (still declared in callisto.toml but no longer in the workspace) has
/// no entry in `base`, so if it were included here and happened to carry
/// a release tag, `base.get(&released[0])` would miss and silently fall
/// back to the `1.0.0` default below, corrupting the alignment base for
/// every live sibling. Accepting a pre-filtered slice instead of the raw
/// `GroupDef` makes that corruption unrepresentable at the call site.
pub fn fixed_group_target(
    live_members: &[PackageId],
    base: &BTreeMap<PackageId, Version>,
    max_sev: Severity,
    tags: &TagIndex,
) -> Result<Version, GraphError> {
    let released: Vec<PackageId> = live_members
        .iter()
        .filter(|id| tags.last_tag(id).is_some())
        .cloned()
        .collect();

    let aligned_base = if !released.is_empty() {
        base.get(&released[0])
            .cloned()
            .unwrap_or_else(|| Version::semver(1, 0, 0))
    } else {
        Version::semver(0, 0, 0)
    };

    let versioning = callisto_format::SemVerVersioning;

    versioning.bump(&aligned_base, max_sev).map_err(GraphError::Bump)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use callisto_model::{GroupKind, GroupName, ManifestRole, PackageId, Version};

    use super::*;
    use crate::config::groups::{GroupDef, GroupMember, GroupTable};
    use crate::napi::NapiTargetsIndex;
    use crate::tags::TagIndex;

    struct EmptyResolver;
    impl crate::resolver::DependencyResolver for EmptyResolver {
        fn packages(&self) -> impl Iterator<Item = &callisto_model::Package> {
            std::iter::empty()
        }
        fn dependencies_of(&self, _id: &PackageId) -> impl Iterator<Item = &callisto_model::DepEdge> {
            std::iter::empty()
        }
        fn dependents_of(&self, _id: &PackageId) -> impl Iterator<Item = &callisto_model::DepEdge> {
            std::iter::empty()
        }
        fn diagnostics(&self) -> &[callisto_model::Diagnostic] {
            &[]
        }
    }

    #[test]
    fn pre_mutation_checks_calls_napi_drift_for_napi_groups() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        let group_name = GroupName("my-lib".to_string());
        let pkg_id = PackageId::Bare("my-lib".to_string());

        // One platform member with a known triple (darwin/arm64 → aarch64-apple-darwin).
        // We do NOT create a platform manifest on disk, so the only diagnostic emitted
        // should be NapiTargetAddedNotInMembers (declared triple not in members).
        let platform_role = ManifestRole::Platform {
            platform: "linux".to_string(),
            arch: "x64".to_string(),
            abi: Some("gnu".to_string()),
        };

        let group = GroupDef {
            name: group_name.clone(),
            kind: GroupKind::Fixed,
            members: vec![
                GroupMember::Package(pkg_id.clone()),
                GroupMember::PlatformManifest {
                    owner: pkg_id.clone(),
                    role: platform_role,
                    path: PathBuf::from("platform/linux-x64-gnu/package.json"),
                    name: "my-lib.linux-x64-gnu".to_string(),
                },
            ],
        };

        let groups = GroupTable::from_groups(vec![group], vec![]);

        // Build the NapiTargetsIndex from a real package.json file —
        let pkg_dir = root.join("my-lib");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("package.json"),
            r#"{"name":"my-lib","napi":{"targets":["aarch64-apple-darwin"]}}"#,
        )
        .unwrap();

        let napi = NapiTargetsIndex::load(&groups, root).expect("load");

        let base: BTreeMap<PackageId, Version> = BTreeMap::new();
        let tags = TagIndex::empty();
        let resolver = EmptyResolver;

        let outcome = pre_mutation_checks(&resolver, &groups, &base, &tags, &napi, root).expect("pre_mutation_checks");

        assert!(
            !outcome.diagnostics.is_empty(),
            "expected at least one napi_drift diagnostic"
        );
    }
}
