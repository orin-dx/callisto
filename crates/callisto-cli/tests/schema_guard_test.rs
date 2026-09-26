//! Guards every report struct's field shape against an accidental schema change. The
//! `StatusReport.pending` field is mandatory (count of packages with a planned bump --
//! `status --check`'s 0/1 exit code doesn't itself signal pending state, so this is how a
//! script detects it). The `tag` and `plan-publish` report schemas left with their commands.
//! Expected sets below were captured from `callisto schema` against the live repository;
//! update them deliberately alongside a `DiagnosticCode` or report field change.

use std::collections::BTreeSet;
use std::process::Command;

fn run_schema(target: &str) -> serde_json::Value {
    let bin = env!("CARGO_BIN_EXE_callisto");
    let out = Command::new(bin)
        .args(["schema", "--type", target])
        .output()
        .expect("callisto schema must run");
    assert!(
        out.status.success(),
        "callisto schema --type {target} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("schema output must be valid JSON")
}

fn required_and_props(schema: &serde_json::Value) -> (BTreeSet<String>, BTreeSet<String>) {
    let required: BTreeSet<String> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let props: BTreeSet<String> = schema["properties"].as_object().unwrap().keys().cloned().collect();
    (required, props)
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn report_struct_field_shapes_are_unchanged() {
    let status_schema = run_schema("status");
    let (req, props) = required_and_props(&status_schema);
    assert_eq!(req, set(&["hasChangesets", "packages", "pending", "schemaVersion"]));
    assert_eq!(
        props,
        set(&["diagnostics", "hasChangesets", "packages", "pending", "schemaVersion"])
    );
}

#[test]
fn diagnostic_code_enum_variants_match_expected_set() {
    let status_schema = run_schema("status");
    let variants: BTreeSet<String> = status_schema["definitions"]["DiagnosticCode"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["enum"][0].as_str().unwrap().to_string())
        .collect();

    let expected = set(&[
        "empty-changeset",
        "empty-summary",
        "unknown-package",
        "invalid-package-name",
        "napi-target-added-not-in-members",
        "napi-target-removed-still-on-disk",
        "napi-coordination-not-yet-supported",
        "range-not-round-trippable",
        "catalog-spec-not-rewritten",
        "tag-glob-non-version-match",
        "changesets-config-key-dropped",
        "pre-major-inference-inert",
        "changelog-section-not-found",
        "changeset-read-error",
        "changeset-parse-failed",
        "git-discovery-failed",
        "init-origin-missing",
        "bare-rule-matches-multiple-ecosystems",
        "unrecognised-platform-triple",
        "publish-target-not-implemented",
        "package-set-matched-nothing",
        "duplicate-platform-triple",
        "changelog-read-error",
        "ambiguous-package-name",
        "platform-package-without-owner",
        "workflow-generation-unsupported",
        "workflow-merge-publishes",
        "package-rule-matched-nothing",
        "product-artifact-owner-without-product",
    ]);

    assert_eq!(
        variants, expected,
        "DiagnosticCode schema variants changed; update this list deliberately"
    );
}

/// The receipt is versioned: a change to its shape without a `SCHEMA_VERSION`
/// bump is a silent break for receipts written by an earlier release. The
/// receipt is at version 2 and its run envelope at version 3 (no run kind, no profile).
#[test]
fn durable_release_wire_shapes_match_their_schema_version() {
    let receipt = run_schema("release-receipt");
    let (req, props) = required_and_props(&receipt);
    let expected = set(&["schemaVersion", "intentDigest", "envelope", "outcomes", "observations"]);
    assert_eq!(req, expected);
    assert_eq!(props, expected);

    let (req, props) = required_and_props(&receipt["definitions"]["ReleaseRunEnvelopeV1"]);
    let mut expected = set(&[
        "schemaVersion",
        "orchestrationRevision",
        "releaseSourceRevision",
        "intentDigest",
    ]);
    assert_eq!(req, expected);
    expected.insert("artifactManifestDigest".to_owned());
    assert_eq!(props, expected);
}

/// The operation role is the durable DAG's vocabulary: a receipt names every
/// operation by it. `forgePublish` is the role that makes publication the last
/// forge step, after every `artifactUpload`.
#[test]
fn release_operation_roles_carry_exactly_their_declared_variants() {
    let receipt = run_schema("release-receipt");
    let variants: BTreeSet<String> = receipt["definitions"]["ReleaseOperationRole"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|variant| {
            variant["properties"]["kind"]["enum"][0]
                .as_str()
                .expect("every role variant names itself")
                .to_owned()
        })
        .collect();
    assert_eq!(
        variants,
        set(&[
            "registryPublish",
            "tag",
            "forgeRelease",
            "artifactUpload",
            "forgePublish",
            "platformPublish",
        ])
    );
}

/// Both observation enums are closed and persisted in receipts, so a
/// reader from an earlier release must be able to name every value it can meet.
/// Adding one is intentional and belongs here; losing one silently is not.
#[test]
fn provider_observation_enums_carry_exactly_their_declared_variants() {
    let receipt = run_schema("release-receipt");
    let variants = |name: &str| -> BTreeSet<String> {
        receipt["definitions"][name]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|variant| {
                variant["enum"]
                    .get(0)
                    .or_else(|| variant["properties"]["kind"]["enum"].get(0))
                    .and_then(serde_json::Value::as_str)
                    .expect("every variant names itself")
                    .to_owned()
            })
            .collect()
    };

    assert_eq!(
        variants("ProviderConflictReason"),
        set(&[
            "localTagDiffers",
            "remoteTagTargetDiffers",
            "unannotatedTag",
            "forgeReleaseDiffers",
            "forgeReleasePrereleaseDiffers",
            "artifactAssetDiffers",
            "duplicateArtifactAsset",
            "registryVersionYanked",
        ])
    );
    assert_eq!(
        variants("ProviderIndeterminateCause"),
        set(&[
            "unsupportedProvider",
            "providerStatus",
            "commandFailed",
            "malformedResponse",
            "timeout",
            "registryVersionUnverified",
            "artifactManifestUnavailable",
        ])
    );
}
