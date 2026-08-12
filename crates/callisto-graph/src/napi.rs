use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use callisto_model::{Diagnostic, DiagnosticCode, DiagnosticSeverity, ManifestRole, StrictFlag};

use crate::config::groups::GroupMember;
use crate::config::{GroupDef, GroupTable};

#[derive(Clone, Debug, Default)]
pub struct NapiTargetsIndex {
    declared: BTreeMap<callisto_model::GroupName, Vec<String>>,
}

impl NapiTargetsIndex {
    pub fn load(groups: &GroupTable, root: &Path) -> Result<Self, callisto_model::ManifestError> {
        let mut declared = BTreeMap::new();
        for g in groups.fixed.values() {
            // Find the first Package member — it is the napi main package.
            let main_id = g.members.iter().find_map(|m| match m {
                GroupMember::Package(id) => Some(id),
                _ => None,
            });
            let Some(main_id) = main_id else {
                continue;
            };

            // Derive the expected package.json path from the package name.
            // Convention: root/<package-name>/package.json
            let pkg_json_path = root.join(main_id.name()).join("package.json");
            if !pkg_json_path.exists() {
                // Group has no napi package.json — skip, not an error.
                continue;
            }

            let content = std::fs::read_to_string(&pkg_json_path).map_err(|e| {
                callisto_model::ManifestError::Read {
                    path: pkg_json_path.clone(),
                    message: e.to_string(),
                }
            })?;

            let val: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
                callisto_model::ManifestError::Parse {
                    path: pkg_json_path.clone(),
                    format: callisto_model::ManifestFormat::PackageJson,
                    message: e.to_string(),
                }
            })?;

            // Only insert when the "napi" key is present.
            if let Some(targets) = val
                .get("napi")
                .and_then(|n| n.get("targets"))
                .and_then(|t| t.as_array())
            {
                let triples: Vec<String> = targets
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
                declared.insert(g.name.clone(), triples);
            }
        }
        Ok(NapiTargetsIndex { declared })
    }

    pub fn declared_for(&self, group: &callisto_model::GroupName) -> Option<&[String]> {
        self.declared.get(group).map(|v| v.as_slice())
    }
}

pub fn napi_drift(group: &GroupDef, declared: &[String], root: &Path) -> Vec<Diagnostic> {
    use crate::config::groups::GroupMemberKind;

    let declared_triples: BTreeSet<String> =
        declared.iter().map(|s| s.trim().to_string()).collect();

    let member_triples: BTreeSet<String> = group
        .members(GroupMemberKind::PlatformManifest)
        .filter_map(|m| match m {
            GroupMember::PlatformManifest { role, .. } => role_to_triple(role),
            _ => None,
        })
        .collect();

    let mut diagnostics = Vec::new();

    // Declared in napi.targets but no corresponding group member.
    for t in declared_triples.difference(&member_triples) {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::NapiTargetAddedNotInMembers,
            severity: DiagnosticSeverity::Warning,
            message: format!(
                "`napi.targets` declares `{t}`, which is not in fixed group `{}`'s members; accept it with `callisto init`",
                group.name
            ),
            package: None,
            path: None,
            governed_by: None,
            escalated_by: Some(StrictFlag::Strict),
        });
    }

    // Present in group members but removed from napi.targets — only warn if
    // the physical manifest file still exists on disk.
    for t in member_triples.difference(&declared_triples) {
        // Find the PlatformManifest whose triple matches t.
        let manifest_path =
            group
                .members(GroupMemberKind::PlatformManifest)
                .find_map(|m| match m {
                    GroupMember::PlatformManifest { role, path, .. } => {
                        if role_to_triple(role).as_deref() == Some(t.as_str()) {
                            Some(root.join(path))
                        } else {
                            None
                        }
                    }
                    _ => None,
                });

        if let Some(abs_path) = manifest_path {
            if abs_path.exists() {
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::NapiTargetRemovedStillOnDisk,
                    severity: DiagnosticSeverity::Warning,
                    message: format!(
                        "fixed group `{}` member `{t}` is no longer in `napi.targets` but its manifest still exists on disk; run `callisto init` to reconcile",
                        group.name
                    ),
                    package: None,
                    path: Some(abs_path),
                    governed_by: None,
                    escalated_by: Some(StrictFlag::Strict),
                });
            }
        }
    }

    diagnostics
}

/// Maps a napi-rs target triple to a `ManifestRole::Platform` describing its
/// platform, architecture, and (for Linux) ABI.
///
/// Returns `None` for any triple not in the known napi-rs target table.
pub fn triple_to_role(triple: &str) -> Option<ManifestRole> {
    let (platform, arch, abi) = match triple {
        "aarch64-apple-darwin" => ("darwin", "arm64", None),
        "x86_64-apple-darwin" => ("darwin", "x64", None),
        "x86_64-unknown-linux-gnu" => ("linux", "x64", Some("gnu")),
        "x86_64-unknown-linux-musl" => ("linux", "x64", Some("musl")),
        "aarch64-unknown-linux-gnu" => ("linux", "arm64", Some("gnu")),
        "aarch64-unknown-linux-musl" => ("linux", "arm64", Some("musl")),
        "x86_64-pc-windows-msvc" => ("win32", "x64", None),
        "i686-pc-windows-msvc" => ("win32", "ia32", None),
        "aarch64-pc-windows-msvc" => ("win32", "arm64", None),
        "armv7-unknown-linux-gnueabihf" => ("linux", "arm", Some("gnueabihf")),
        "x86_64-unknown-freebsd" => ("freebsd", "x64", None),
        "aarch64-linux-android" => ("android", "arm64", None),
        "armv7-linux-androideabi" => ("android", "arm", None),
        "riscv64gc-unknown-linux-gnu" => ("linux", "riscv64", Some("gnu")),
        "powerpc64le-unknown-linux-gnu" => ("linux", "ppc64", Some("gnu")),
        "s390x-unknown-linux-gnu" => ("linux", "s390x", Some("gnu")),
        "wasm32-wasip1" => ("wasi", "wasm32", None),
        "wasm32-unknown-unknown" => ("unknown", "wasm32", None),
        _ => return None,
    };
    Some(ManifestRole::Platform {
        platform: platform.to_string(),
        arch: arch.to_string(),
        abi: abi.map(str::to_string),
    })
}

/// Reverse of `triple_to_role`: maps a `ManifestRole::Platform` back to the
/// canonical napi-rs target triple. Returns `None` for roles not in the table
/// (including `Canonical` and `Lockfile` roles).
pub fn role_to_triple(role: &ManifestRole) -> Option<String> {
    let ManifestRole::Platform {
        platform,
        arch,
        abi,
    } = role
    else {
        return None;
    };
    let triple = match (platform.as_str(), arch.as_str(), abi.as_deref()) {
        ("darwin", "arm64", None) => "aarch64-apple-darwin",
        ("darwin", "x64", None) => "x86_64-apple-darwin",
        ("linux", "x64", Some("gnu")) => "x86_64-unknown-linux-gnu",
        ("linux", "x64", Some("musl")) => "x86_64-unknown-linux-musl",
        ("linux", "arm64", Some("gnu")) => "aarch64-unknown-linux-gnu",
        ("linux", "arm64", Some("musl")) => "aarch64-unknown-linux-musl",
        ("win32", "x64", None) => "x86_64-pc-windows-msvc",
        ("win32", "ia32", None) => "i686-pc-windows-msvc",
        ("win32", "arm64", None) => "aarch64-pc-windows-msvc",
        ("linux", "arm", Some("gnueabihf")) => "armv7-unknown-linux-gnueabihf",
        ("freebsd", "x64", None) => "x86_64-unknown-freebsd",
        ("android", "arm64", None) => "aarch64-linux-android",
        ("android", "arm", None) => "armv7-linux-androideabi",
        ("linux", "riscv64", Some("gnu")) => "riscv64gc-unknown-linux-gnu",
        ("linux", "ppc64", Some("gnu")) => "powerpc64le-unknown-linux-gnu",
        ("linux", "s390x", Some("gnu")) => "s390x-unknown-linux-gnu",
        ("wasi", "wasm32", None) => "wasm32-wasip1",
        ("unknown", "wasm32", None) => "wasm32-unknown-unknown",
        _ => return None,
    };
    Some(triple.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use callisto_model::{DiagnosticCode, GroupKind, GroupName, ManifestRole, PackageId};

    use super::*;
    use crate::config::groups::{GroupDef, GroupMember, GroupTable};

    const KNOWN_TRIPLES: &[&str] = &[
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-gnu",
        "x86_64-unknown-linux-musl",
        "aarch64-unknown-linux-gnu",
        "aarch64-unknown-linux-musl",
        "x86_64-pc-windows-msvc",
        "i686-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
        "armv7-unknown-linux-gnueabihf",
        "x86_64-unknown-freebsd",
        "aarch64-linux-android",
        "armv7-linux-androideabi",
        "riscv64gc-unknown-linux-gnu",
        "powerpc64le-unknown-linux-gnu",
        "s390x-unknown-linux-gnu",
        "wasm32-wasip1",
        "wasm32-unknown-unknown",
    ];

    #[test]
    fn triple_to_role_known_triples_round_trip() {
        for &t in KNOWN_TRIPLES {
            let role = triple_to_role(t)
                .unwrap_or_else(|| panic!("triple_to_role returned None for known triple `{t}`"));
            let back = role_to_triple(&role).unwrap_or_else(|| {
                panic!("role_to_triple returned None for role derived from `{t}`")
            });
            assert_eq!(
                back, t,
                "round-trip failed for `{t}`: role_to_triple produced `{back}`"
            );
        }
    }

    #[test]
    fn triple_to_role_unknown_returns_none() {
        assert!(
            triple_to_role("x86_64-unknown-openbsd").is_none(),
            "expected None for unrecognized triple"
        );
    }

    fn make_platform_role(platform: &str, arch: &str, abi: Option<&str>) -> ManifestRole {
        ManifestRole::Platform {
            platform: platform.to_string(),
            arch: arch.to_string(),
            abi: abi.map(str::to_string),
        }
    }

    fn make_group(
        name: &str,
        main_pkg: &str,
        platform_members: Vec<(&str, ManifestRole, PathBuf)>,
    ) -> GroupDef {
        let mut members = vec![GroupMember::Package(PackageId::Bare(main_pkg.to_string()))];
        for (pm_name, role, path) in platform_members {
            members.push(GroupMember::PlatformManifest {
                owner: PackageId::Bare(main_pkg.to_string()),
                role,
                path,
                name: pm_name.to_string(),
            });
        }
        GroupDef {
            name: GroupName(name.to_string()),
            kind: GroupKind::Fixed,
            members,
        }
    }

    #[test]
    fn napi_targets_index_loads_targets_from_package_json() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        // Create a package directory named "my-lib" with package.json
        let pkg_dir = root.join("my-lib");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("package.json"),
            r#"{"name":"my-lib","napi":{"targets":["aarch64-apple-darwin"]}}"#,
        )
        .unwrap();

        let group_name = GroupName("my-lib-group".to_string());
        let group = GroupDef {
            name: group_name.clone(),
            kind: GroupKind::Fixed,
            members: vec![GroupMember::Package(PackageId::Bare("my-lib".to_string()))],
        };
        let groups = GroupTable::from_groups(vec![group], vec![]);

        let index = NapiTargetsIndex::load(&groups, root).expect("load should succeed");
        let declared = index
            .declared_for(&group_name)
            .expect("declared_for should return Some");
        assert_eq!(declared, &["aarch64-apple-darwin"]);
    }

    #[test]
    fn napi_drift_no_drift_produces_no_diagnostics() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        // Create a platform manifest file on disk.
        let pm_path = PathBuf::from("platform/darwin-arm64/package.json");
        let abs_pm = root.join(&pm_path);
        std::fs::create_dir_all(abs_pm.parent().unwrap()).unwrap();
        std::fs::write(&abs_pm, r#"{"name":"my-lib-darwin-arm64"}"#).unwrap();

        let group = make_group(
            "my-lib",
            "my-lib",
            vec![(
                "my-lib.darwin-arm64",
                make_platform_role("darwin", "arm64", None),
                pm_path,
            )],
        );

        let declared = vec!["aarch64-apple-darwin".to_string()];
        let diags = napi_drift(&group, &declared, root);
        assert!(
            diags.is_empty(),
            "expected no diagnostics for matching declared and members, got: {diags:?}"
        );
    }

    #[test]
    fn napi_drift_added_target_produces_added_diagnostic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        // Group has no platform members for this triple.
        let group = make_group("my-lib", "my-lib", vec![]);

        let declared = vec!["aarch64-apple-darwin".to_string()];
        let diags = napi_drift(&group, &declared, root);
        assert_eq!(diags.len(), 1, "expected one diagnostic, got: {diags:?}");
        assert_eq!(
            diags[0].code,
            DiagnosticCode::NapiTargetAddedNotInMembers,
            "expected NapiTargetAddedNotInMembers"
        );
    }

    #[test]
    fn napi_drift_removed_target_with_file_produces_removed_diagnostic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        // Member triple exists in group but NOT in declared.
        let pm_path = PathBuf::from("platform/darwin-arm64/package.json");
        let abs_pm = root.join(&pm_path);
        std::fs::create_dir_all(abs_pm.parent().unwrap()).unwrap();
        std::fs::write(&abs_pm, r#"{"name":"my-lib-darwin-arm64"}"#).unwrap();

        let group = make_group(
            "my-lib",
            "my-lib",
            vec![(
                "my-lib.darwin-arm64",
                make_platform_role("darwin", "arm64", None),
                pm_path,
            )],
        );

        // declared is empty — the member triple is "removed"
        let declared: Vec<String> = vec![];
        let diags = napi_drift(&group, &declared, root);
        assert_eq!(diags.len(), 1, "expected one diagnostic, got: {diags:?}");
        assert_eq!(
            diags[0].code,
            DiagnosticCode::NapiTargetRemovedStillOnDisk,
            "expected NapiTargetRemovedStillOnDisk"
        );
    }
}
