//! AC-13 guard: none of this track's fixes may change any report struct's field shape.
//! The only permitted schema delta is the additive DiagnosticCode::ChangelogReadError
//! variant. Expected sets below were captured from `callisto schema` against the live,
//! unmodified repository before this track's fixes were implemented.

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
    let props: BTreeSet<String> = schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    (required, props)
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn ac13_report_struct_field_shapes_are_unchanged() {
    let tag_schema = run_schema("tag");
    let (req, props) = required_and_props(&tag_schema);
    assert_eq!(req, set(&["createdTags", "schemaVersion"]));
    assert_eq!(props, set(&["createdTags", "diagnostics", "schemaVersion"]));

    let created_tag = &tag_schema["definitions"]["CreatedTag"];
    let (req, props) = required_and_props(created_tag);
    assert_eq!(req, set(&["package", "sha", "tagName"]));
    assert_eq!(props, req);

    let status_schema = run_schema("status");
    let (req, props) = required_and_props(&status_schema);
    assert_eq!(req, set(&["packages", "schemaVersion"]));
    assert_eq!(props, set(&["diagnostics", "packages", "schemaVersion"]));

    let plan_schema = run_schema("plan-publish");
    let (req, props) = required_and_props(&plan_schema);
    assert_eq!(
        req,
        set(&[
            "npmMainPackages",
            "npmPlatformPackages",
            "releases",
            "rustCrates",
            "schemaVersion"
        ])
    );
    assert_eq!(
        props,
        set(&[
            "diagnostics",
            "npmMainPackages",
            "npmPlatformPackages",
            "pypiPackages",
            "releases",
            "rustCrates",
            "schemaVersion"
        ])
    );

    let release_entry = &plan_schema["definitions"]["ReleaseEntry"];
    let (req, props) = required_and_props(release_entry);
    assert_eq!(req, set(&["package", "sha", "tagName"]));
    assert_eq!(
        props,
        set(&["changelogSection", "package", "sha", "tagName"])
    );
}

#[test]
fn ac13_diagnostic_code_enum_gains_only_changelog_read_error() {
    let tag_schema = run_schema("tag");
    let variants: BTreeSet<String> = tag_schema["definitions"]["DiagnosticCode"]["oneOf"]
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
        "graph-edge-disagreement",
        "range-not-round-trippable",
        "catalog-spec-not-rewritten",
        "tag-glob-non-version-match",
        "changesets-config-key-dropped",
        "pre-major-inference-inert",
        "changelog-section-not-found",
        "changeset-read-error",
        "git-discovery-failed",
        "bare-rule-matches-multiple-ecosystems",
        "unrecognised-platform-triple",
        "publish-target-not-implemented",
        "package-set-matched-nothing",
        "duplicate-platform-triple",
        "changelog-read-error",
    ]);

    assert_eq!(
        variants, expected,
        "DiagnosticCode schema must gain exactly one new variant: changelog-read-error"
    );
}
