use serde::{Deserialize, Serialize};

use super::{ArtifactManifestV1, ReleaseIntentV1, ReleaseReceiptV1};

/// `callisto release --format json` report: flattens [`ReleaseReceiptV1`] so its own
/// fields reach stdout unchanged, through the one envelope every command shares.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseReport {
    #[serde(flatten)]
    pub receipt: ReleaseReceiptV1,
}

impl crate::report::Report for ReleaseReport {
    const COMMAND: &'static str = "release";

    fn schema_version(&self) -> u32 {
        u32::from(ReleaseReceiptV1::SCHEMA_VERSION)
    }

    fn diagnostics(&self) -> &[crate::Diagnostic] {
        &[]
    }
}

/// `callisto release`/`callisto release --dry-run --format json` report when no package
/// has an unreleased version.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseNothingReport {
    pub nothing_to_release: bool,
}

impl crate::report::Report for ReleaseNothingReport {
    const COMMAND: &'static str = "release";

    fn schema_version(&self) -> u32 {
        crate::report::SCHEMA_VERSION
    }

    fn diagnostics(&self) -> &[crate::Diagnostic] {
        &[]
    }
}

/// `callisto release --dry-run --format json` report when a release is planned: same
/// intent-plus-diagnostics shape as [`ReleasePlanReport`], under `release`'s own `command`.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseDryRunReport {
    #[serde(flatten)]
    pub intent: ReleaseIntentV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<crate::Diagnostic>,
}

impl crate::report::Report for ReleaseDryRunReport {
    const COMMAND: &'static str = "release";

    fn schema_version(&self) -> u32 {
        u32::from(ReleaseIntentV1::SCHEMA_VERSION)
    }

    fn diagnostics(&self) -> &[crate::Diagnostic] {
        &self.diagnostics
    }
}

/// `callisto release execute --format json` report: same payload shape as
/// [`ReleaseReport`], under `release execute`'s own `command` value.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseExecuteReport {
    #[serde(flatten)]
    pub receipt: ReleaseReceiptV1,
}

impl crate::report::Report for ReleaseExecuteReport {
    const COMMAND: &'static str = "release execute";

    fn schema_version(&self) -> u32 {
        u32::from(ReleaseReceiptV1::SCHEMA_VERSION)
    }

    fn diagnostics(&self) -> &[crate::Diagnostic] {
        &[]
    }
}

/// `release plan` JSON report: the intent plus derivation diagnostics, which the intent file never carries.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleasePlanReport {
    #[serde(flatten)]
    pub intent: ReleaseIntentV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<crate::Diagnostic>,
}

impl crate::report::Report for ReleasePlanReport {
    const COMMAND: &'static str = "release plan";

    fn schema_version(&self) -> u32 {
        u32::from(ReleaseIntentV1::SCHEMA_VERSION)
    }

    fn diagnostics(&self) -> &[crate::Diagnostic] {
        &self.diagnostics
    }
}

/// `callisto release artifact-manifest --format json` report: flattens [`ArtifactManifestV1`].
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseArtifactManifestReport {
    #[serde(flatten)]
    pub manifest: ArtifactManifestV1,
}

impl crate::report::Report for ReleaseArtifactManifestReport {
    const COMMAND: &'static str = "release artifact-manifest";

    fn schema_version(&self) -> u32 {
        u32::from(ArtifactManifestV1::SCHEMA_VERSION)
    }

    fn diagnostics(&self) -> &[crate::Diagnostic] {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use crate::Ecosystem;

    use super::super::test_support::test_intent;
    use super::*;
    use crate::release::*;

    /// `ReleasePlanReport` must flatten `ReleaseIntentV1`'s own fields (not nest them
    /// under an `intent` key) and add a `diagnostics` array only when non-empty --
    /// the one JSON envelope `output::emit_report` gives every command alike.
    #[test]
    fn release_plan_report_flattens_intent_and_omits_empty_diagnostics() {
        let package = ReleasePackageId::new(Ecosystem::Cargo, "demo").unwrap();
        let version = Version::semver(1, 0, 0);
        let tag = ReleaseOperation::tag(package, version, vec![]).unwrap();
        let intent = test_intent(
            Ok(ReleaseInputSnapshotV1::new(SourceIdentity::git_commit("a".repeat(40)).unwrap(), vec![]).unwrap()),
            ExecutionTrustProfileV1::GitCommit,
            vec![tag],
        )
        .unwrap();
        let report = ReleasePlanReport {
            intent: intent.clone(),
            diagnostics: Vec::new(),
        };

        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["digest"], serde_json::to_value(intent.digest()).unwrap());
        assert!(value.get("intent").is_none(), "must flatten, not nest: {value}");
        assert!(
            value.get("diagnostics").is_none(),
            "empty diagnostics must be omitted: {value}"
        );
        assert_eq!(
            crate::report::Report::schema_version(&report),
            u32::from(ReleaseIntentV1::SCHEMA_VERSION)
        );
    }
}
