//! Manifest reading and MatrixReport assembly for `callisto matrix`.
//! Private helpers only -- the public entry point is
//! `crate::commands::matrix::matrix`.

/// Per-triple CI scheduling attributes for the 18 triples
/// `crate::napi::triple_to_role` recognises today. This table is NEW code:
/// `triple_to_role`'s `ManifestRole::Platform` carries only platform/arch/abi
/// and has no concept of hostRunner/useCross, so this cannot be derived from
/// it. Returns `None` for any triple `triple_to_role` does not recognise --
/// callers must treat that as the AC-011 diagnostic path, never a silent
/// default.
pub(crate) fn triple_host_runner_use_cross(triple: &str) -> Option<(&'static str, bool)> {
    Some(match triple {
        "aarch64-apple-darwin" => ("macos-latest", false),
        "x86_64-apple-darwin" => ("macos-13", false),
        "x86_64-unknown-linux-gnu" => ("ubuntu-latest", false),
        "x86_64-unknown-linux-musl" => ("ubuntu-latest", true),
        "aarch64-unknown-linux-gnu" => ("ubuntu-latest", true),
        "aarch64-unknown-linux-musl" => ("ubuntu-latest", true),
        "x86_64-pc-windows-msvc" => ("windows-latest", false),
        "i686-pc-windows-msvc" => ("windows-latest", false),
        "aarch64-pc-windows-msvc" => ("windows-latest", false),
        "armv7-unknown-linux-gnueabihf" => ("ubuntu-latest", true),
        "x86_64-unknown-freebsd" => ("ubuntu-latest", true),
        "aarch64-linux-android" => ("ubuntu-latest", true),
        "armv7-linux-androideabi" => ("ubuntu-latest", true),
        "riscv64gc-unknown-linux-gnu" => ("ubuntu-latest", true),
        "powerpc64le-unknown-linux-gnu" => ("ubuntu-latest", true),
        "s390x-unknown-linux-gnu" => ("ubuntu-latest", true),
        "wasm32-wasip1" => ("ubuntu-latest", false),
        "wasm32-unknown-unknown" => ("ubuntu-latest", false),
        _ => return None,
    })
}

/// AC-013: artifactName is always the literal "native-" concatenated with
/// the triple string, for every recognised triple.
pub(crate) fn artifact_name_for_triple(triple: &str) -> String {
    format!("native-{triple}")
}

use std::path::Path;

use callisto_model::{ManifestError, ManifestFormat, ManifestRole, PlatformTarget};

use crate::error::GraphError;
use crate::napi::triple_to_role;

/// Result of reading the `napi.targets` field from a package.json. Distinct
/// from `Option<Vec<String>>` so AC-001b (present-but-empty vs absent) is a
/// type-level distinction, not a magic-empty-vec convention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NapiTargetsField {
    Absent,
    Present(Vec<String>),
}

/// Reads `napi.targets` directly from `pkg_json_path`. A missing `napi` key
/// or missing `napi.targets` key is `Absent` (AC-003: no platformTargets
/// entry). A present `napi.targets` that is not a JSON array of strings is a
/// hard error (AC-010c) -- unlike `NapiTargetsIndex::load`, which silently
/// drops non-array values.
pub(crate) fn read_napi_targets(pkg_json_path: &Path) -> Result<NapiTargetsField, GraphError> {
    let content = std::fs::read_to_string(pkg_json_path).map_err(|e| {
        GraphError::Manifest(ManifestError::Read {
            path: pkg_json_path.to_path_buf(),
            message: e.to_string(),
        })
    })?;
    let val: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        GraphError::Manifest(ManifestError::Parse {
            path: pkg_json_path.to_path_buf(),
            format: ManifestFormat::PackageJson,
            message: e.to_string(),
        })
    })?;

    let Some(napi) = val.get("napi") else {
        return Ok(NapiTargetsField::Absent);
    };
    let Some(targets) = napi.get("targets") else {
        return Ok(NapiTargetsField::Absent);
    };

    let arr = targets.as_array().ok_or_else(|| {
        GraphError::Manifest(ManifestError::Parse {
            path: pkg_json_path.to_path_buf(),
            format: ManifestFormat::PackageJson,
            message: "napi.targets must be a JSON array of strings".to_string(),
        })
    })?;

    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item.as_str().ok_or_else(|| {
            GraphError::Manifest(ManifestError::Parse {
                path: pkg_json_path.to_path_buf(),
                format: ManifestFormat::PackageJson,
                message: "napi.targets entries must all be strings".to_string(),
            })
        })?;
        out.push(s.to_string());
    }
    Ok(NapiTargetsField::Present(out))
}

/// Builds a PlatformTarget for `triple`, combining `triple_to_role`'s
/// platform/arch/abi with this module's hostRunner/useCross/artifactName
/// table. Returns `None` when `triple` is not recognised by either --
/// callers must route that case to an UnrecognisedPlatformTriple diagnostic
/// (AC-011) rather than treating it as an error.
pub(crate) fn build_platform_target(
    triple: &str,
    package_dir: &str,
    package_name: &str,
) -> Option<PlatformTarget> {
    let ManifestRole::Platform {
        platform,
        arch,
        abi,
    } = triple_to_role(triple)?
    else {
        return None;
    };
    let (host_runner, use_cross) = triple_host_runner_use_cross(triple)?;
    Some(PlatformTarget {
        triple: triple.to_string(),
        platform,
        arch,
        abi,
        host_runner: host_runner.to_string(),
        use_cross,
        artifact_name: artifact_name_for_triple(triple),
        package_dir: package_dir.to_string(),
        package_name: package_name.to_string(),
    })
}

/// Reads `[tool.maturin].targets` directly from `pyproject_path`. Returns
/// `Ok(None)` when the table or field is absent (AC-003: no platformTargets
/// entry). A present value that is not a TOML array of strings is a hard
/// error (AC-010c).
pub(crate) fn read_maturin_targets(
    pyproject_path: &Path,
) -> Result<Option<Vec<String>>, GraphError> {
    let content = std::fs::read_to_string(pyproject_path).map_err(|e| {
        GraphError::Manifest(ManifestError::Read {
            path: pyproject_path.to_path_buf(),
            message: e.to_string(),
        })
    })?;
    let val: toml::Value = content.parse().map_err(|e: toml::de::Error| {
        GraphError::Manifest(ManifestError::Parse {
            path: pyproject_path.to_path_buf(),
            format: ManifestFormat::PyprojectToml,
            message: e.to_string(),
        })
    })?;

    let Some(targets) = val
        .get("tool")
        .and_then(|t| t.get("maturin"))
        .and_then(|m| m.get("targets"))
    else {
        return Ok(None);
    };

    let arr = targets.as_array().ok_or_else(|| {
        GraphError::Manifest(ManifestError::Parse {
            path: pyproject_path.to_path_buf(),
            format: ManifestFormat::PyprojectToml,
            message: "[tool.maturin].targets must be an array of strings".to_string(),
        })
    })?;

    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item.as_str().ok_or_else(|| {
            GraphError::Manifest(ManifestError::Parse {
                path: pyproject_path.to_path_buf(),
                format: ManifestFormat::PyprojectToml,
                message: "[tool.maturin].targets entries must all be strings".to_string(),
            })
        })?;
        out.push(s.to_string());
    }
    Ok(Some(out))
}

/// Reads `engines.node` directly from `pkg_json_path` as a raw string.
/// `Ok(None)` when absent; `Err` when present but not a JSON string.
pub(crate) fn read_engines_node(pkg_json_path: &Path) -> Result<Option<String>, GraphError> {
    let content = std::fs::read_to_string(pkg_json_path).map_err(|e| {
        GraphError::Manifest(ManifestError::Read {
            path: pkg_json_path.to_path_buf(),
            message: e.to_string(),
        })
    })?;
    let val: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        GraphError::Manifest(ManifestError::Parse {
            path: pkg_json_path.to_path_buf(),
            format: ManifestFormat::PackageJson,
            message: e.to_string(),
        })
    })?;
    let Some(node) = val.get("engines").and_then(|e| e.get("node")) else {
        return Ok(None);
    };
    let s = node.as_str().ok_or_else(|| {
        GraphError::Manifest(ManifestError::Parse {
            path: pkg_json_path.to_path_buf(),
            format: ManifestFormat::PackageJson,
            message: "engines.node must be a string".to_string(),
        })
    })?;
    Ok(Some(s.to_string()))
}

/// Reads `requires-python` directly from `pyproject_path` as a raw string.
/// `Ok(None)` when absent; `Err` when present but not a TOML string.
pub(crate) fn read_requires_python(pyproject_path: &Path) -> Result<Option<String>, GraphError> {
    let content = std::fs::read_to_string(pyproject_path).map_err(|e| {
        GraphError::Manifest(ManifestError::Read {
            path: pyproject_path.to_path_buf(),
            message: e.to_string(),
        })
    })?;
    let val: toml::Value = content.parse().map_err(|e: toml::de::Error| {
        GraphError::Manifest(ManifestError::Parse {
            path: pyproject_path.to_path_buf(),
            format: ManifestFormat::PyprojectToml,
            message: e.to_string(),
        })
    })?;
    let Some(rp) = val.get("project").and_then(|p| p.get("requires-python")) else {
        return Ok(None);
    };
    let s = rp.as_str().ok_or_else(|| {
        GraphError::Manifest(ManifestError::Parse {
            path: pyproject_path.to_path_buf(),
            format: ManifestFormat::PyprojectToml,
            message: "requires-python must be a string".to_string(),
        })
    })?;
    Ok(Some(s.to_string()))
}

use callisto_model::{PackageId, PlatformTargetKind};

/// Reads napi.targets and [tool.maturin].targets from `package_dir_abs`
/// (the package's on-disk directory) and determines which single source (if
/// any) declares platform targets.
///
/// - Neither declared: `Ok(None)` (AC-003).
/// - Both declared: `Err(GraphError::ConflictingPlatformTargetSources)` (AC-017).
/// - Exactly one declared (even as an explicitly empty array, AC-001b):
///   `Ok(Some((kind, source, triples)))`.
pub(crate) fn select_platform_target_source(
    package_dir_abs: &Path,
    package_id: &PackageId,
) -> Result<Option<(PlatformTargetKind, String, Vec<String>)>, GraphError> {
    let napi_path = package_dir_abs.join("package.json");
    let napi_field = if napi_path.exists() {
        read_napi_targets(&napi_path)?
    } else {
        NapiTargetsField::Absent
    };

    let maturin_path = package_dir_abs.join("pyproject.toml");
    let maturin_targets = if maturin_path.exists() {
        read_maturin_targets(&maturin_path)?
    } else {
        None
    };

    match (napi_field, maturin_targets) {
        (NapiTargetsField::Present(_), Some(_)) => {
            Err(GraphError::ConflictingPlatformTargetSources {
                package: package_id.clone(),
                napi_source: "napi.targets",
                maturin_source: "[tool.maturin].targets",
            })
        }
        (NapiTargetsField::Present(triples), None) => Ok(Some((
            PlatformTargetKind::Napi,
            "napi.targets".to_string(),
            triples,
        ))),
        (NapiTargetsField::Absent, Some(triples)) => Ok(Some((
            PlatformTargetKind::Maturin,
            "[tool.maturin].targets".to_string(),
            triples,
        ))),
        (NapiTargetsField::Absent, None) => Ok(None),
    }
}

use callisto_model::{Diagnostic, DiagnosticCode, DiagnosticSeverity, PlatformTargetGroup};

/// Calls `select_platform_target_source` and, when a source is present,
/// builds a PlatformTarget for every declared triple (AC-001, AC-002),
/// excluding any triple `build_platform_target` does not recognise and
/// pushing an `UnrecognisedPlatformTriple` warning Diagnostic for each one
/// instead (AC-011). `group.targets` is sorted ascending by triple string
/// (AC-009) before returning.
pub(crate) fn assemble_platform_target_group(
    package_dir_abs: &Path,
    package_dir_rel: &str,
    package_name: &str,
    package_id: &PackageId,
) -> Result<(Option<PlatformTargetGroup>, Vec<Diagnostic>), GraphError> {
    let Some((kind, source, triples)) = select_platform_target_source(package_dir_abs, package_id)?
    else {
        return Ok((None, Vec::new()));
    };

    let mut targets = Vec::new();
    let mut diagnostics = Vec::new();
    for triple in &triples {
        match build_platform_target(triple, package_dir_rel, package_name) {
            Some(t) => targets.push(t),
            None => diagnostics.push(Diagnostic {
                code: DiagnosticCode::UnrecognisedPlatformTriple,
                severity: DiagnosticSeverity::Warning,
                message: format!(
                    "package `{package_name}` declares unrecognised platform triple `{triple}` in `{source}`"
                ),
                package: Some(package_id.clone()),
                path: None,
                escalated_by: None,
                governed_by: None,
            }),
        }
    }
    targets.sort_by(|a, b| a.triple.cmp(&b.triple));

    Ok((
        Some(PlatformTargetGroup {
            kind,
            source,
            targets,
        }),
        diagnostics,
    ))
}

use std::collections::BTreeMap;
use std::path::PathBuf;

use callisto_model::{MatrixReport, RuntimeEcosystem, RuntimeVersionEntry};

/// One registered package's identity and on-disk location, adapted from a
/// real `Workspace` by `crate::commands::matrix::matrix` (or built directly
/// by tests).
pub(crate) struct MatrixPackageInput {
    pub id: PackageId,
    /// Absolute path to the package's directory (used for manifest reads).
    pub dir_abs: PathBuf,
    /// Workspace-root-relative path string, stored verbatim on each
    /// PlatformTarget.package_dir.
    pub dir_rel: String,
    pub name: String,
}

/// Reads engines.node (npm) and requires-python (python) from `package_dir_abs`,
/// in that order, so callers preserve the npm-before-python ordering AC-005b
/// requires without a separate sort step.
pub(crate) fn assemble_runtime_versions(
    package_dir_abs: &Path,
) -> Result<Vec<RuntimeVersionEntry>, GraphError> {
    let mut entries = Vec::new();

    let pkg_json = package_dir_abs.join("package.json");
    if pkg_json.exists() {
        if let Some(range) = read_engines_node(&pkg_json)? {
            entries.push(RuntimeVersionEntry {
                ecosystem: RuntimeEcosystem::Npm,
                field: "engines.node".to_string(),
                range,
            });
        }
    }

    let pyproject = package_dir_abs.join("pyproject.toml");
    if pyproject.exists() {
        if let Some(range) = read_requires_python(&pyproject)? {
            entries.push(RuntimeVersionEntry {
                ecosystem: RuntimeEcosystem::Python,
                field: "requires-python".to_string(),
                range,
            });
        }
    }

    Ok(entries)
}

/// Assembles the full MatrixReport for `packages`. Map keys are each
/// package's bare name (AC-009's BTreeMap ordering comes from this for free);
/// a package contributes no platformTargets entry when it declares neither
/// napi.targets nor [tool.maturin].targets, and no runtimeVersions entry when
/// it declares neither engines.node nor requires-python.
pub(crate) fn build_matrix_report(
    packages: &[MatrixPackageInput],
) -> Result<MatrixReport, GraphError> {
    let mut platform_targets = BTreeMap::new();
    let mut runtime_versions = BTreeMap::new();
    let mut diagnostics = Vec::new();

    for pkg in packages {
        let (group, diags) =
            assemble_platform_target_group(&pkg.dir_abs, &pkg.dir_rel, &pkg.name, &pkg.id)?;
        if let Some(group) = group {
            platform_targets.insert(pkg.name.clone(), group);
        }
        diagnostics.extend(diags);

        let rv = assemble_runtime_versions(&pkg.dir_abs)?;
        if !rv.is_empty() {
            runtime_versions.insert(pkg.name.clone(), rv);
        }
    }

    Ok(MatrixReport {
        schema_version: callisto_model::SCHEMA_VERSION,
        platform_targets,
        runtime_versions,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::PackageId;

    fn pkg_id(name: &str) -> PackageId {
        PackageId::Bare(name.to_string())
    }

    fn input(name: &str, dir: &std::path::Path) -> MatrixPackageInput {
        MatrixPackageInput {
            id: pkg_id(name),
            dir_abs: dir.to_path_buf(),
            dir_rel: name.to_string(),
            name: name.to_string(),
        }
    }

    /// AC-003: no packages declare anything -> the empty-report shape.
    #[test]
    fn build_matrix_report_empty_workspace_produces_empty_report() {
        let report = build_matrix_report(&[]).unwrap();
        assert_eq!(report.schema_version, callisto_model::SCHEMA_VERSION);
        assert!(report.platform_targets.is_empty());
        assert!(report.runtime_versions.is_empty());
        assert!(report.diagnostics.is_empty());
    }

    /// AC-009: platformTargets and runtimeVersions keys are ordered
    /// lexicographically by package name across 3+ packages.
    #[test]
    fn build_matrix_report_orders_packages_lexicographically() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["zeta", "alpha", "mid"] {
            let dir = tmp.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("package.json"),
                r#"{"napi":{"targets":["aarch64-apple-darwin"]}}"#,
            )
            .unwrap();
        }
        let inputs = vec![
            input("zeta", &tmp.path().join("zeta")),
            input("alpha", &tmp.path().join("alpha")),
            input("mid", &tmp.path().join("mid")),
        ];
        let report = build_matrix_report(&inputs).unwrap();
        let keys: Vec<&String> = report.platform_targets.keys().collect();
        assert_eq!(keys, vec!["alpha", "mid", "zeta"]);
    }

    /// AC-005b: a package with both engines.node and requires-python gets a
    /// two-element runtimeVersions array, npm before python, and this is not
    /// an error.
    #[test]
    fn build_matrix_report_dual_manifest_runtime_versions_npm_before_python() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("dual-pkg");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"engines":{"node":">=20.0.0"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("pyproject.toml"),
            "[project]\nrequires-python = \">=3.9\"\n",
        )
        .unwrap();

        let report = build_matrix_report(&[input("dual-pkg", &dir)]).unwrap();
        let entries = report
            .runtime_versions
            .get("dual-pkg")
            .expect("dual-pkg must have runtimeVersions entries");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].ecosystem, RuntimeEcosystem::Npm);
        assert_eq!(entries[0].range, ">=20.0.0");
        assert_eq!(entries[1].ecosystem, RuntimeEcosystem::Python);
        assert_eq!(entries[1].range, ">=3.9");
    }

    /// AC-017: a package.json with napi.targets AND a pyproject.toml with
    /// [tool.maturin].targets in the same directory is a hard error.
    #[test]
    fn select_platform_target_source_conflicting_sources_errors() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("package.json"),
            r#"{"napi":{"targets":["aarch64-apple-darwin"]}}"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("pyproject.toml"),
            "[tool.maturin]\ntargets = [\"x86_64-unknown-linux-gnu\"]\n",
        )
        .unwrap();

        let err = select_platform_target_source(tmp.path(), &pkg_id("native-mod")).unwrap_err();
        match err {
            GraphError::ConflictingPlatformTargetSources {
                package,
                napi_source,
                maturin_source,
            } => {
                assert_eq!(package, pkg_id("native-mod"));
                assert_eq!(napi_source, "napi.targets");
                assert_eq!(maturin_source, "[tool.maturin].targets");
            }
            other => panic!("expected ConflictingPlatformTargetSources, got {other:?}"),
        }
    }

    /// AC-003 (per-package slice): neither manifest declares platform
    /// targets -> None.
    #[test]
    fn select_platform_target_source_no_manifests_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let result = select_platform_target_source(tmp.path(), &pkg_id("plain-pkg")).unwrap();
        assert!(result.is_none());
    }

    /// AC-001b: an explicitly empty napi.targets = [] must be distinguishable
    /// from the field being absent entirely.
    #[test]
    fn read_napi_targets_distinguishes_absent_from_present_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let absent_path = tmp.path().join("absent.json");
        std::fs::write(&absent_path, r#"{"name":"pkg"}"#).unwrap();
        assert_eq!(
            read_napi_targets(&absent_path).unwrap(),
            NapiTargetsField::Absent
        );

        let empty_path = tmp.path().join("empty.json");
        std::fs::write(&empty_path, r#"{"napi":{"targets":[]}}"#).unwrap();
        assert_eq!(
            read_napi_targets(&empty_path).unwrap(),
            NapiTargetsField::Present(vec![])
        );

        let populated_path = tmp.path().join("populated.json");
        std::fs::write(
            &populated_path,
            r#"{"napi":{"targets":["aarch64-apple-darwin","x86_64-unknown-linux-gnu"]}}"#,
        )
        .unwrap();
        assert_eq!(
            read_napi_targets(&populated_path).unwrap(),
            NapiTargetsField::Present(vec![
                "aarch64-apple-darwin".to_string(),
                "x86_64-unknown-linux-gnu".to_string()
            ])
        );
    }

    /// AC-010b: malformed JSON syntax must be a hard read error naming the path.
    #[test]
    fn read_napi_targets_malformed_json_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad.json");
        std::fs::write(&path, r#"{"napi":{"targets":["a",]}}"#).unwrap(); // trailing comma
        let err = read_napi_targets(&path).unwrap_err();
        assert!(
            format!("{err}").contains(&path.display().to_string()),
            "error must name the malformed path: {err}"
        );
    }

    /// AC-010c: napi.targets present but not a JSON array (a bare string) must
    /// be a hard error, not silently treated as absent.
    #[test]
    fn read_napi_targets_non_array_value_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("wrong_type.json");
        std::fs::write(&path, r#"{"napi":{"targets":"aarch64-apple-darwin"}}"#).unwrap();
        let err = read_napi_targets(&path).unwrap_err();
        assert!(
            format!("{err}").contains(&path.display().to_string()),
            "error must name the malformed path: {err}"
        );
    }

    /// AC-012: table-driven assertion of all 18 (hostRunner, useCross) pairs.
    #[test]
    fn triple_host_runner_use_cross_matches_all_18_triples() {
        let expected: &[(&str, &str, bool)] = &[
            ("aarch64-apple-darwin", "macos-latest", false),
            ("x86_64-apple-darwin", "macos-13", false),
            ("x86_64-unknown-linux-gnu", "ubuntu-latest", false),
            ("x86_64-unknown-linux-musl", "ubuntu-latest", true),
            ("aarch64-unknown-linux-gnu", "ubuntu-latest", true),
            ("aarch64-unknown-linux-musl", "ubuntu-latest", true),
            ("x86_64-pc-windows-msvc", "windows-latest", false),
            ("i686-pc-windows-msvc", "windows-latest", false),
            ("aarch64-pc-windows-msvc", "windows-latest", false),
            ("armv7-unknown-linux-gnueabihf", "ubuntu-latest", true),
            ("x86_64-unknown-freebsd", "ubuntu-latest", true),
            ("aarch64-linux-android", "ubuntu-latest", true),
            ("armv7-linux-androideabi", "ubuntu-latest", true),
            ("riscv64gc-unknown-linux-gnu", "ubuntu-latest", true),
            ("powerpc64le-unknown-linux-gnu", "ubuntu-latest", true),
            ("s390x-unknown-linux-gnu", "ubuntu-latest", true),
            ("wasm32-wasip1", "ubuntu-latest", false),
            ("wasm32-unknown-unknown", "ubuntu-latest", false),
        ];
        assert_eq!(
            expected.len(),
            18,
            "sanity check: table must cover exactly 18 triples"
        );
        for &(triple, host_runner, use_cross) in expected {
            let (got_runner, got_cross) =
                triple_host_runner_use_cross(triple).unwrap_or_else(|| {
                    panic!("triple_host_runner_use_cross returned None for known triple `{triple}`")
                });
            assert_eq!(
                got_runner, host_runner,
                "hostRunner mismatch for `{triple}`"
            );
            assert_eq!(got_cross, use_cross, "useCross mismatch for `{triple}`");
        }
    }

    /// AC-013: artifactName is always "native-" + triple.
    #[test]
    fn artifact_name_is_native_prefixed_triple() {
        assert_eq!(
            artifact_name_for_triple("aarch64-apple-darwin"),
            "native-aarch64-apple-darwin"
        );
        assert_eq!(
            artifact_name_for_triple("x86_64-unknown-linux-musl"),
            "native-x86_64-unknown-linux-musl"
        );
    }

    /// An unrecognised triple must return None, not panic or fall back to a
    /// default -- callers use this to drive the AC-011 diagnostic path.
    #[test]
    fn triple_host_runner_use_cross_unknown_triple_returns_none() {
        assert!(triple_host_runner_use_cross("sparc64-unknown-linux-gnu").is_none());
    }

    /// AC-001 (mapping slice) + AC-014 (abi null on non-linux platforms):
    /// build_platform_target must combine triple_to_role's platform/arch/abi
    /// with the CI table's hostRunner/useCross/artifactName.
    #[test]
    fn build_platform_target_combines_role_and_ci_table() {
        let t = build_platform_target("aarch64-apple-darwin", "packages/native-mod", "native-mod")
            .expect("aarch64-apple-darwin must be recognised");
        assert_eq!(t.triple, "aarch64-apple-darwin");
        assert_eq!(t.platform, "darwin");
        assert_eq!(t.arch, "arm64");
        assert_eq!(t.abi, None, "darwin targets must serialize abi as null");
        assert_eq!(t.host_runner, "macos-latest");
        assert!(!t.use_cross);
        assert_eq!(t.artifact_name, "native-aarch64-apple-darwin");
        assert_eq!(t.package_dir, "packages/native-mod");
        assert_eq!(t.package_name, "native-mod");
    }

    /// AC-014: a linux triple must carry a non-null abi string.
    #[test]
    fn build_platform_target_linux_triple_has_non_null_abi() {
        let t = build_platform_target("x86_64-unknown-linux-gnu", "pkg-dir", "pkg-name")
            .expect("x86_64-unknown-linux-gnu must be recognised");
        assert_eq!(t.abi, Some("gnu".to_string()));
    }

    /// An unrecognised triple must return None -- this is the hook the AC-011
    /// diagnostic path (added in a later task) relies on.
    #[test]
    fn build_platform_target_unrecognised_triple_returns_none() {
        assert!(build_platform_target("sparc64-unknown-linux-gnu", "dir", "name").is_none());
    }

    /// AC-014: one triple per remaining platform family (win32, freebsd,
    /// android, wasi, and unknown/wasm32-unknown-unknown) must all carry a
    /// null abi -- extending the darwin/linux coverage above to the rest of
    /// the recognised triples' platform families.
    #[test]
    fn build_platform_target_null_abi_families_table_driven() {
        let cases: &[(&str, &str)] = &[
            ("x86_64-pc-windows-msvc", "win32"),
            ("x86_64-unknown-freebsd", "freebsd"),
            ("aarch64-linux-android", "android"),
            ("wasm32-wasip1", "wasi"),
            ("wasm32-unknown-unknown", "unknown"),
        ];
        for &(triple, expected_platform) in cases {
            let t = build_platform_target(triple, "dir", "name")
                .unwrap_or_else(|| panic!("{triple} must be recognised"));
            assert_eq!(
                t.platform, expected_platform,
                "platform mismatch for `{triple}`"
            );
            assert_eq!(t.abi, None, "abi must be null for `{triple}`");
        }
    }

    /// AC-002 precursor: [tool.maturin].targets reads as a plain Vec<String>
    /// when present, None when absent.
    #[test]
    fn read_maturin_targets_reads_present_and_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let absent_path = tmp.path().join("absent.toml");
        std::fs::write(&absent_path, "[project]\nname = \"pkg\"\n").unwrap();
        assert_eq!(read_maturin_targets(&absent_path).unwrap(), None);

        let present_path = tmp.path().join("present.toml");
        std::fs::write(
            &present_path,
            "[tool.maturin]\ntargets = [\"x86_64-unknown-linux-gnu\", \"aarch64-apple-darwin\"]\n",
        )
        .unwrap();
        assert_eq!(
            read_maturin_targets(&present_path).unwrap(),
            Some(vec![
                "x86_64-unknown-linux-gnu".to_string(),
                "aarch64-apple-darwin".to_string()
            ])
        );
    }

    /// [tool.maturin].targets = [] (present, explicitly empty) must be
    /// distinguishable from absent -- Some(vec![]), not None.
    #[test]
    fn read_maturin_targets_present_empty_is_some_empty_vec() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("empty.toml");
        std::fs::write(&path, "[tool.maturin]\ntargets = []\n").unwrap();
        assert_eq!(read_maturin_targets(&path).unwrap(), Some(vec![]));
    }

    /// AC-010: malformed TOML syntax must be a hard error naming the path.
    #[test]
    fn read_maturin_targets_malformed_toml_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad.toml");
        std::fs::write(&path, "[tool.maturin]\ntargets = [\"unterminated\n").unwrap();
        let err = read_maturin_targets(&path).unwrap_err();
        assert!(
            format!("{err}").contains(&path.display().to_string()),
            "error must name the malformed path: {err}"
        );
    }

    /// AC-010c: [tool.maturin].targets present but not an array must be a
    /// hard error.
    #[test]
    fn read_maturin_targets_non_array_value_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("wrong_type.toml");
        std::fs::write(
            &path,
            "[tool.maturin]\ntargets = \"x86_64-unknown-linux-gnu\"\n",
        )
        .unwrap();
        let err = read_maturin_targets(&path).unwrap_err();
        assert!(
            format!("{err}").contains(&path.display().to_string()),
            "error must name the malformed path: {err}"
        );
    }

    /// AC-004: engines.node reads as a raw string; absent is None.
    #[test]
    fn read_engines_node_reads_present_and_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let present = tmp.path().join("present.json");
        std::fs::write(&present, r#"{"engines":{"node":">=20.0.0"}}"#).unwrap();
        assert_eq!(
            read_engines_node(&present).unwrap(),
            Some(">=20.0.0".to_string())
        );

        let absent = tmp.path().join("absent.json");
        std::fs::write(&absent, r#"{"name":"pkg"}"#).unwrap();
        assert_eq!(read_engines_node(&absent).unwrap(), None);
    }

    /// AC-010c: engines.node present but not a string (e.g. a number) is a
    /// hard error.
    #[test]
    fn read_engines_node_non_string_value_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("wrong_type.json");
        std::fs::write(&path, r#"{"engines":{"node":20}}"#).unwrap();
        assert!(read_engines_node(&path).is_err());
    }

    /// AC-005: requires-python reads as a raw string; absent is None.
    #[test]
    fn read_requires_python_reads_present_and_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let present = tmp.path().join("present.toml");
        std::fs::write(&present, "[project]\nrequires-python = \">=3.9\"\n").unwrap();
        assert_eq!(
            read_requires_python(&present).unwrap(),
            Some(">=3.9".to_string())
        );

        let absent = tmp.path().join("absent.toml");
        std::fs::write(&absent, "[project]\nname = \"pkg\"\n").unwrap();
        assert_eq!(read_requires_python(&absent).unwrap(), None);
    }

    /// AC-010c: requires-python present but not a string is a hard error.
    #[test]
    fn read_requires_python_non_string_value_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("wrong_type.toml");
        std::fs::write(&path, "[project]\nrequires-python = 39\n").unwrap();
        assert!(read_requires_python(&path).is_err());
    }

    /// AC-011: an unrecognised triple is excluded from targets[] and reported
    /// as a warning diagnostic naming the triple and the package; recognised
    /// triples in the same declaration remain present.
    #[test]
    fn assemble_platform_target_group_unrecognised_triple_produces_diagnostic() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("package.json"),
            r#"{"napi":{"targets":["sparc64-unknown-linux-gnu","aarch64-apple-darwin"]}}"#,
        )
        .unwrap();

        let (group, diagnostics) = assemble_platform_target_group(
            tmp.path(),
            "native-mod",
            "native-mod",
            &pkg_id("native-mod"),
        )
        .unwrap();
        let group = group.expect("group must be present");
        assert_eq!(
            group.targets.len(),
            1,
            "only the recognised triple must remain"
        );
        assert_eq!(group.targets[0].triple, "aarch64-apple-darwin");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].code,
            DiagnosticCode::UnrecognisedPlatformTriple
        );
        assert!(diagnostics[0].message.contains("sparc64-unknown-linux-gnu"));
        assert_eq!(diagnostics[0].package, Some(pkg_id("native-mod")));
    }

    /// AC-001b: napi.targets = [] (present, explicitly empty) produces a
    /// present-but-empty group, not None.
    #[test]
    fn assemble_platform_target_group_empty_array_is_present_not_absent() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("package.json"),
            r#"{"napi":{"targets":[]}}"#,
        )
        .unwrap();

        let (group, diagnostics) = assemble_platform_target_group(
            tmp.path(),
            "native-mod",
            "native-mod",
            &pkg_id("native-mod"),
        )
        .unwrap();
        let group = group.expect("an explicitly empty napi.targets must still produce a group");
        assert_eq!(group.kind, PlatformTargetKind::Napi);
        assert_eq!(group.source, "napi.targets");
        assert!(group.targets.is_empty());
        assert!(diagnostics.is_empty());
    }
}
