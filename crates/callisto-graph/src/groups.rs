use std::collections::BTreeMap;
use std::path::Path;

use crate::config::groups::GroupTable;
use crate::error::GraphError;
use crate::napi::{napi_drift, NapiTargetsIndex};
use crate::resolver::DependencyResolver;
use crate::tags::TagIndex;
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

        // napi.targets drift cross-check.
        if let Some(declared) = napi.declared_for(&g.name) {
            outcome.diagnostics.extend(napi_drift(g, declared, root));
        }
    }

    Ok(outcome)
}

/// Computes `base`'s next version under `severity`: grammar-aware (SemVer vs
/// PEP 440, via `callisto_format::versioning_for`) and, in pre-release mode,
/// anchored on the `initialVersions` entry pinned in `pre.json` rather than
/// the live on-disk version. Shared by `cascade::bump_target` (per-package
/// bumps) and `fixed_group_target` (group-aligned bumps) so the two paths
/// can never diverge on grammar or pre-release handling the way
/// `fixed_group_target` once did by hardcoding `SemVerVersioning` and
/// ignoring `pre` entirely.
///
/// Guards against ever returning a version that sorts behind `base`: a
/// miscomputed alignment base upstream (e.g. the `1.0.0`/`0.0.0` defaults
/// this function replaces) must surface as an error here rather than silently
/// write a downgrade to a manifest.
pub fn versioned_bump(
    package: &PackageId,
    base: &Version,
    severity: Severity,
    pre: Option<&callisto_format::PreState>,
) -> Result<Version, GraphError> {
    let versioning = callisto_format::versioning_for(base.grammar())
        .ok_or(callisto_format::BumpError::UnsupportedGrammar {
            grammar: base.grammar(),
        })
        .map_err(GraphError::Bump)?;

    // The regression guard compares against the version actually being bumped
    // from: in pre-release mode that's the pinned `initialVersions` anchor,
    // not the live on-disk prerelease, which may already be deeper into a
    // *different*, larger-severity pre-release cycle than the pinned baseline
    // (e.g. on-disk "2.0.0-next.0" pinned at "1.0.0" legitimately bumps Minor
    // to "1.1.0-next.0" -- smaller than on-disk, but not a regression).
    let (bumped_from, next) = match pre {
        Some(pre) if pre.mode == callisto_format::PreMode::Pre => {
            let pinned_base = pre.initial_versions.get(crate::pre_json_key(package)).unwrap_or(base);
            let next = versioning
                .bump_prerelease(pinned_base, severity, &pre.tag, base)
                .map_err(GraphError::Bump)?;
            (pinned_base.clone(), next)
        }
        _ => {
            let next = versioning.bump(base, severity).map_err(GraphError::Bump)?;
            (base.clone(), next)
        }
    };

    if matches!(Version::compare(&next, &bumped_from), Ok(std::cmp::Ordering::Less)) {
        return Err(GraphError::VersionRegression {
            package: package.clone(),
            from: bumped_from,
            to: next,
        });
    }

    Ok(next)
}

/// Computes a fixed group's shared alignment target.
///
/// `live_members` must already be filtered to package ids present in
/// `base` (see `solve_cascade`'s Track-1 block) -- a stale group member
/// (still declared in callisto.toml but no longer in the workspace) has
/// no entry in `base`, so if it were included here and happened to carry
/// a release tag, `base.get(&released[0])` would miss.
///
/// The alignment base is the tagged member's on-disk version when the
/// group has a released member (a member absent from `base` there is an
/// error, not a silent `1.0.0` default -- a live sibling would otherwise
/// align against a fabricated version), or otherwise the highest on-disk
/// version among the untagged live members (not a hardcoded `0.0.0`,
/// which made every untagged fixed-group bump a downgrade).
pub fn fixed_group_target(
    group: &GroupName,
    live_members: &[PackageId],
    base: &BTreeMap<PackageId, Version>,
    max_sev: Severity,
    tags: &TagIndex,
    pre: Option<&callisto_format::PreState>,
) -> Result<Version, GraphError> {
    let released: Vec<&PackageId> = live_members.iter().filter(|id| tags.last_tag(id).is_some()).collect();

    let (anchor, aligned_base) = if !released.is_empty() {
        let anchor = released[0];
        let v = base
            .get(anchor)
            .cloned()
            .ok_or_else(|| GraphError::FixedGroupTaggedMemberMissingBase {
                group: group.clone(),
                member: anchor.clone(),
            })?;
        (anchor.clone(), v)
    } else {
        let mut untagged = live_members
            .iter()
            .filter_map(|id| base.get(id).map(|v| (id.clone(), v.clone())));
        let mut best = untagged
            .next()
            .ok_or_else(|| GraphError::FixedGroupEmpty { group: group.clone() })?;
        for (id, v) in untagged {
            match Version::compare(&v, &best.1) {
                Ok(std::cmp::Ordering::Greater) => best = (id, v),
                Ok(_) => {}
                Err(_) => {
                    return Err(GraphError::GroupGrammarMismatch {
                        group: group.clone(),
                        members: live_members
                            .iter()
                            .filter_map(|m| base.get(m).map(|v| (m.clone(), v.clone())))
                            .collect(),
                    });
                }
            }
        }
        best
    };

    versioned_bump(&anchor, &aligned_base, max_sev, pre)
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

    /// Regression: an untagged fixed group's alignment base must be the
    /// highest on-disk member version, not the hardcoded `0.0.0` the old
    /// code used -- which turned every untagged fixed-group Minor bump into
    /// a downgrade (1.0.0 + minor -> 0.1.0).
    #[test]
    fn fixed_group_target_untagged_base_is_highest_ondisk_not_zero() {
        let pkg_core = PackageId::Bare("core".to_string());
        let pkg_app = PackageId::Bare("app".to_string());
        let group = GroupName("g".to_string());

        let mut base = BTreeMap::new();
        base.insert(pkg_core.clone(), Version::semver(1, 0, 0));
        base.insert(pkg_app.clone(), Version::semver(0, 5, 0));

        let live_members = vec![pkg_core.clone(), pkg_app.clone()];
        let tags = TagIndex::empty();

        let target =
            fixed_group_target(&group, &live_members, &base, Severity::Minor, &tags, None).expect("must compute");

        assert_eq!(
            target.render(),
            "1.1.0",
            "untagged base must be the highest on-disk member version (1.0.0), not 0.0.0"
        );
    }

    /// Regression: fixed-group alignment must honor the base version's own
    /// grammar (via `callisto_format::versioning_for`) instead of always
    /// bumping with `SemVerVersioning`, which corrupted PEP 440 fixed
    /// groups (E035: "bump_version requires a SemVer version").
    #[test]
    fn fixed_group_target_honors_pep440_grammar() {
        let pkg = PackageId::Bare("pya".to_string());
        let group = GroupName("g".to_string());

        let mut base = BTreeMap::new();
        base.insert(
            pkg.clone(),
            Version::parse("1.0.0", callisto_model::VersionGrammar::Pep440).unwrap(),
        );
        let live_members = vec![pkg.clone()];
        let tags = TagIndex::empty();

        let target = fixed_group_target(&group, &live_members, &base, Severity::Minor, &tags, None)
            .expect("PEP 440 base must not error as NotSemVer");

        assert_eq!(target.render(), "1.1.0");
    }

    /// Regression: fixed-group alignment must honor pre-release mode via
    /// `pre.json`'s `initialVersions`, producing a `-<tag>.N` target instead
    /// of silently finalizing to a stable version.
    #[test]
    fn fixed_group_target_honors_pre_mode() {
        let pkg = PackageId::Bare("core".to_string());
        let group = GroupName("g".to_string());

        let mut base = BTreeMap::new();
        base.insert(pkg.clone(), Version::semver(1, 0, 0));
        let live_members = vec![pkg.clone()];
        let tags = TagIndex::empty();

        let mut initial_versions = indexmap::IndexMap::new();
        initial_versions.insert("core".to_string(), Version::semver(1, 0, 0));
        let pre = callisto_format::PreState {
            mode: callisto_format::PreMode::Pre,
            tag: "beta".to_string(),
            initial_versions,
            changesets: Vec::new(),
        };

        let target = fixed_group_target(&group, &live_members, &base, Severity::Minor, &tags, Some(&pre))
            .expect("must compute a pre-release target");

        assert_eq!(
            target.render(),
            "1.1.0-beta.0",
            "pre mode must produce a pre-release target, not a stable 1.1.0"
        );
    }
}
