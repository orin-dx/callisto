use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Diagnostic, Report};

/// Top-level report output from `callisto matrix --format json`.
///
/// BTreeMap guarantees lexicographic key ordering by registered package name
/// (PackageId::name()) without a separate sort step (AC-009).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MatrixReport {
    pub schema_version: u32,
    pub platform_targets: BTreeMap<String, PlatformTargetGroup>,
    pub runtime_versions: BTreeMap<String, Vec<RuntimeVersionEntry>>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl Report for MatrixReport {
    const COMMAND: &'static str = "matrix";

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

/// One package's native-build target group (napi or maturin -- never both;
/// see GraphError::ConflictingPlatformTargetSources).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlatformTargetGroup {
    pub kind: PlatformTargetKind,
    /// Raw manifest field name ("napi.targets" or "[tool.maturin].targets"),
    /// used for audit output only -- not a filesystem path.
    pub source: String,
    /// Sorted ascending by triple string before serialization (AC-009).
    pub targets: Vec<PlatformTarget>,
}

/// Deliberately omits a DotnetAot variant; out of scope for this spec.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PlatformTargetKind {
    Napi,
    Maturin,
}

/// One platform build target within a PlatformTargetGroup. No `rid` field
/// (napi/maturin only, no dotnet-aot).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlatformTarget {
    pub triple: String,
    pub platform: String,
    pub arch: String,
    pub abi: Option<String>,
    pub host_runner: String,
    pub use_cross: bool,
    pub artifact_name: String,
    /// Workspace-root-relative.
    pub package_dir: String,
    pub package_name: String,
}

/// One package's runtime-version constraint, sourced from engines.node (npm)
/// or requires-python (python/PyPI). `range` is the raw, unvalidated manifest
/// string. No targetFrameworks field (dotnet-only, out of scope).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeVersionEntry {
    pub ecosystem: RuntimeEcosystem,
    pub field: String,
    pub range: String,
}

/// Which manifest field a [`RuntimeVersionEntry`] was read from. Exactly two variants —
/// `callisto matrix` reads `engines.node` from `package.json` or `requires-python` from
/// `pyproject.toml` and nothing else; a third ecosystem (e.g. a future `.NET`
/// `TargetFramework` field) is deliberately out of scope, not merely unimplemented, so
/// deserializing an unrecognized ecosystem string is a schema error rather than silently
/// accepted (§M.12.7 — this module's own test suite pins this).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeEcosystem {
    Npm,
    Python,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AC-003 (type-level slice): an empty MatrixReport with no diagnostics
    /// serializes to exactly {"schemaVersion":1,"platformTargets":{},"runtimeVersions":{}}
    /// with no "diagnostics" key present.
    #[test]
    fn empty_matrix_report_serializes_with_no_diagnostics_key() {
        let report = MatrixReport {
            schema_version: 1,
            platform_targets: std::collections::BTreeMap::new(),
            runtime_versions: std::collections::BTreeMap::new(),
            diagnostics: Vec::new(),
        };
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "schemaVersion": 1,
                "platformTargets": {},
                "runtimeVersions": {}
            })
        );
    }

    /// AC-015: PlatformTargetKind has exactly two variants (napi, maturin) --
    /// deserializing a "dotnet-aot" kind must fail.
    #[test]
    fn platform_target_kind_rejects_unknown_dotnet_aot_variant() {
        let raw = serde_json::json!({
            "kind": "dotnet-aot",
            "source": "whatever",
            "targets": []
        });
        let result: Result<PlatformTargetGroup, _> = serde_json::from_value(raw);
        assert!(
            result.is_err(),
            "expected deserialization of kind=dotnet-aot to fail"
        );
    }

    /// AC-015: RuntimeEcosystem has exactly two variants (npm, python) --
    /// deserializing an "dotnet" ecosystem must fail.
    #[test]
    fn runtime_ecosystem_rejects_unknown_dotnet_variant() {
        let raw = serde_json::json!({
            "ecosystem": "dotnet",
            "field": "whatever",
            "range": "whatever"
        });
        let result: Result<RuntimeVersionEntry, _> = serde_json::from_value(raw);
        assert!(
            result.is_err(),
            "expected deserialization of ecosystem=dotnet to fail"
        );
    }
}
