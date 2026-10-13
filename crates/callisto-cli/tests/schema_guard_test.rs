//! Validates serialized payloads against published schemas and guards exact wire vocabulary.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use callisto_model::format::{parse_changeset, PreState};
use callisto_model::{
    ArtifactDigest, BumpReason, BumpRecord, CommitSha, ConfigKey, DepKind, Diagnostic, DiagnosticCode,
    DiagnosticSeverity, ExecutionTrustProfileV1, GitHubRepository, InitReport, LockfileRefreshResult, MatrixReport,
    OperationEvent, PlatformTarget, PlatformTargetGroup, PlatformTargetKind, ProviderEvidenceV1, ProviderObservationV1,
    RegistryBindingDigest, RegistryBindingId, ReleaseDecisionEntry, ReleaseDecisionV1, ReleaseExecutionStateV1,
    ReleaseInclusionReason, ReleaseInputSnapshotV1, ReleaseIntentV1, ReleaseOperation, ReleasePackageId,
    ReleasePackageInputV1, ReleasePrDecisionV2, ReleasePrPullRequestV2, ReleasePrSnapshotV2, ReleaseReceiptV1,
    ReleaseRunEnvelopeV1, ReleaseTrigger, RuntimeEcosystem, RuntimeVersionEntry, SemanticInputDigest, Severity,
    SnapshotReport, SourceIdentity, StatusPackageRecord, StatusReport, StrictFlag, TagName, Version, VersionGrammar,
    VersionReport,
};
use serde_json::{json, Value};

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

fn assert_schema_accepts(schema: &Value, payload: &Value) {
    let validator = jsonschema::validator_for(schema).expect("published schema must compile");
    let errors: Vec<_> = validator.iter_errors(payload).map(|error| error.to_string()).collect();
    assert!(
        errors.is_empty(),
        "schema rejected serialized payload {payload}: {errors:?}"
    );
}

fn assert_nested_mutations_rejected(schema: &Value, payload: &Value, mutations: &[(&str, Value)]) {
    assert_schema_accepts(schema, payload);
    let validator = jsonschema::validator_for(schema).expect("published schema must compile");
    for (pointer, replacement) in mutations {
        let mut invalid = payload.clone();
        *invalid
            .pointer_mut(pointer)
            .unwrap_or_else(|| panic!("mutation must reach an existing nested value at {pointer}")) =
            replacement.clone();
        assert!(
            !validator.is_valid(&invalid),
            "schema accepted invalid nested value at {pointer}: {invalid}"
        );
    }
}

fn package() -> callisto_model::PackageId {
    callisto_model::PackageId::parse("npm/@example/widget").unwrap()
}

fn diagnostics() -> Vec<Diagnostic> {
    vec![Diagnostic {
        code: DiagnosticCode::EmptyChangeset,
        severity: DiagnosticSeverity::Warning,
        message: "Changeset has no package entries".into(),
        package: Some(package()),
        path: Some(".changeset/quiet-widget.md".into()),
        escalated_by: Some(StrictFlag::Strict),
        governed_by: Some(ConfigKey::VALIDATION_ALLOW_EMPTY_CHANGESETS),
    }]
}

fn bump() -> BumpRecord {
    BumpRecord {
        package: package(),
        from: Version::semver(1, 2, 3),
        to: Version::parse("1.3.0-beta.1", VersionGrammar::SemVer).unwrap(),
        severity: Severity::Minor,
        governed_by: Some(ConfigKey::CASCADE_MODE),
        reason: Some(BumpReason::Cascade {
            via: callisto_model::PackageId::parse("cargo/widget-core").unwrap(),
            dep_kind: DepKind::Runtime,
            spec: "^1.2.3".into(),
            dependency_to: Version::semver(1, 3, 0),
        }),
    }
}

#[test]
fn status_schema_validates_nested_serialized_payload() {
    let payload = serde_json::to_value(StatusReport {
        schema_version: 1,
        has_changesets: true,
        pending: 1,
        packages: vec![StatusPackageRecord {
            package: package(),
            current_version: Version::semver(1, 2, 3),
            last_tag: Some(TagName::parse("widget@1.2.2").unwrap()),
            last_released_version: Some(Version::semver(1, 2, 2)),
            pending_severity: Some(Severity::Minor),
            changed_since_last_tag: true,
            release_trigger: ReleaseTrigger::Changeset,
            pending_changesets: vec!["quiet-widget".into()],
        }],
        diagnostics: diagnostics(),
    })
    .unwrap();
    assert_eq!(payload["packages"][0]["package"], "npm/@example/widget");
    assert_eq!(payload["packages"][0]["currentVersion"], "1.2.3");
    assert_nested_mutations_rejected(
        &run_schema("status"),
        &payload,
        &[
            ("/packages/0/package", json!({"Bare": "widget"})),
            ("/packages/0/currentVersion", json!({"raw": "1.2.3"})),
            ("/packages/0/lastReleasedVersion", json!(12)),
            ("/packages/0/pendingChangesets/0", json!(false)),
            ("/diagnostics/0/package", json!(["widget"])),
            ("/diagnostics/0/severity", json!("fatal")),
        ],
    );
}

#[test]
fn version_and_snapshot_schemas_validate_nested_serialized_payloads() {
    let version = serde_json::to_value(VersionReport {
        schema_version: 1,
        bumps: vec![bump()],
        lockfile_refresh_results: Some(vec![LockfileRefreshResult {
            filename: "package-lock.json".into(),
            refresh_command: "npm install --package-lock-only".into(),
            success: true,
            exit_code: Some(0),
        }]),
        diagnostics: diagnostics(),
    })
    .unwrap();
    let snapshot = serde_json::to_value(SnapshotReport {
        schema_version: 1,
        snapshot_tag: "canary".into(),
        bumps: vec![bump()],
        diagnostics: diagnostics(),
    })
    .unwrap();
    for (target, payload) in [("version", &version), ("snapshot", &snapshot)] {
        assert_eq!(payload["bumps"][0]["to"], "1.3.0-beta.1");
        assert_nested_mutations_rejected(
            &run_schema(target),
            payload,
            &[
                ("/bumps/0/from", json!({"raw": "1.2.3"})),
                ("/bumps/0/to", json!(123)),
                ("/bumps/0/package", json!({"name": "widget"})),
                ("/bumps/0/reason/via", json!(false)),
                ("/bumps/0/reason/dependency_to", json!(["1.3.0"])),
                ("/bumps/0/reason/kind", json!("unknownReason")),
            ],
        );
    }
    assert_nested_mutations_rejected(
        &run_schema("version"),
        &version,
        &[("/lockfileRefreshResults/0/exitCode", json!("0"))],
    );
}

#[test]
fn init_schema_validates_nonempty_files_and_diagnostics() {
    let payload = serde_json::to_value(InitReport {
        schema_version: 1,
        initialized: true,
        config_path: "callisto.toml".into(),
        config: "[workspace]\n".into(),
        files: vec!["callisto.toml".into(), ".changeset/README.md".into()],
        diagnostics: diagnostics(),
    })
    .unwrap();
    assert_nested_mutations_rejected(
        &run_schema("init"),
        &payload,
        &[
            ("/files/0", json!({"path": "callisto.toml"})),
            ("/diagnostics/0/code", json!("unknown-code")),
            ("/diagnostics/0/path", json!(42)),
        ],
    );
}

#[test]
fn changeset_schema_validates_fields_from_a_parsed_document() {
    let changeset = parse_changeset("---\n\"npm/@example/widget\": minor\n---\nImprove widget rendering\n").unwrap();
    // Changeset's durable format is Markdown; the model has no JSON serializer.
    let entries: Vec<_> = changeset
        .entries
        .iter()
        .map(|entry| json!({"name": entry.name, "severity": entry.severity}))
        .collect();
    let payload = json!({"entries": entries, "summary": changeset.summary});
    assert_nested_mutations_rejected(
        &run_schema("changeset"),
        &payload,
        &[
            ("/entries/0/name", json!({"name": "widget"})),
            ("/entries/0/severity", json!("urgent")),
        ],
    );
}

#[test]
fn pre_state_schema_validates_native_version_strings() {
    let mut state = PreState::entering(
        "beta",
        [
            ("npm/@example/widget".into(), Version::semver(1, 2, 3)),
            (
                "pypi/widget-py".into(),
                Version::parse("1.2.3rc1", VersionGrammar::Pep440).unwrap(),
            ),
        ],
    );
    state.changesets.push("quiet-widget".into());
    for state in [state.clone(), state.exiting()] {
        let payload = serde_json::to_value(state).unwrap();
        assert_eq!(payload["initialVersions"]["pypi/widget-py"], "1.2.3rc1");
        assert_nested_mutations_rejected(
            &run_schema("pre-state"),
            &payload,
            &[
                ("/initialVersions/npm~1@example~1widget", json!({"raw": "1.2.3"})),
                ("/initialVersions/pypi~1widget-py", json!(123)),
                ("/changesets/0", json!(false)),
            ],
        );
    }
}

#[test]
fn matrix_schema_validates_nonempty_target_and_runtime_maps() {
    let payload = serde_json::to_value(MatrixReport {
        schema_version: 1,
        platform_targets: BTreeMap::from([(
            "npm/@example/widget".into(),
            PlatformTargetGroup {
                kind: PlatformTargetKind::Napi,
                source: "napi.targets".into(),
                targets: vec![PlatformTarget {
                    triple: "x86_64-unknown-linux-gnu".into(),
                    platform: "linux".into(),
                    arch: "x64".into(),
                    abi: Some("gnu".into()),
                    host_runner: "ubuntu-latest".into(),
                    use_cross: true,
                    artifact_name: "widget.node".into(),
                    package_dir: "packages/widget".into(),
                    package_name: "@example/widget".into(),
                    manifest_path: Some("crates/widget/Cargo.toml".into()),
                }],
            },
        )]),
        runtime_versions: BTreeMap::from([(
            "npm/@example/widget".into(),
            vec![RuntimeVersionEntry {
                ecosystem: RuntimeEcosystem::Npm,
                field: "engines.node".into(),
                range: ">=20".into(),
            }],
        )]),
        diagnostics: diagnostics(),
    })
    .unwrap();
    assert_nested_mutations_rejected(
        &run_schema("matrix"),
        &payload,
        &[
            (
                "/platformTargets/npm~1@example~1widget/targets/0/useCross",
                json!("true"),
            ),
            ("/platformTargets/npm~1@example~1widget/kind", json!("unknown")),
            ("/runtimeVersions/npm~1@example~1widget/0/ecosystem", json!("cargo")),
            ("/runtimeVersions/npm~1@example~1widget/0/range", json!(20)),
        ],
    );
}

fn receipt() -> ReleaseReceiptV1 {
    let package = ReleasePackageId::parse("cargo/widget-core").unwrap();
    let version = Version::semver(1, 3, 0);
    let revision = CommitSha::parse(&"a".repeat(40)).unwrap();
    let registry = RegistryBindingId::new("cratesIo", RegistryBindingDigest::parse(&"b".repeat(64)).unwrap()).unwrap();
    let publish = ReleaseOperation::registry_publish(package.clone(), version.clone(), registry, vec![]).unwrap();
    let tag = ReleaseOperation::tag(package.clone(), version.clone(), vec![publish.id().clone()]).unwrap();
    let decision = ReleaseDecisionV1::new(vec![ReleaseDecisionEntry {
        package: package.clone(),
        target_version: version.clone(),
        reasons: vec![ReleaseInclusionReason::Changeset],
    }])
    .unwrap();
    let snapshot = ReleaseInputSnapshotV1::new(
        SourceIdentity::GitCommit { sha: revision.clone() },
        vec![ReleasePackageInputV1 {
            package,
            fingerprint: SemanticInputDigest::parse(&"c".repeat(64)).unwrap(),
        }],
    )
    .unwrap();
    let intent = ReleaseIntentV1::new(
        decision,
        snapshot,
        ExecutionTrustProfileV1::GitCommit,
        vec![publish.clone(), tag.clone()],
        vec![],
    )
    .unwrap();
    let envelope = ReleaseRunEnvelopeV1::new(revision.clone(), &intent, None).unwrap();
    let mut state = ReleaseExecutionStateV1::new(&intent, envelope).unwrap();
    for (operation, evidence) in [
        (
            publish,
            ProviderEvidenceV1::RegistryVersion {
                version,
                checksum: Some(ArtifactDigest::parse(&"d".repeat(64)).unwrap()),
                yanked: Some(false),
            },
        ),
        (
            tag,
            ProviderEvidenceV1::GitTag {
                peeled_commit: revision,
            },
        ),
    ] {
        let observation = ProviderObservationV1::Exact { evidence };
        state
            .apply(
                operation.id(),
                &OperationEvent::ObservedExactBeforeEffect {
                    evidence: observation.exact_evidence().unwrap(),
                },
            )
            .unwrap();
    }
    ReleaseReceiptV1::from_state(&intent, &state).unwrap()
}

#[test]
fn receipt_schema_validates_durable_operations_evidence_and_digests() {
    let payload = serde_json::to_value(receipt()).unwrap();
    assert!(payload.get("command").is_none());
    assert!(payload.get("dryRun").is_none());
    assert_eq!(payload["outcomes"][0]["operation"]["package"], "cargo/widget-core");
    assert_eq!(payload["outcomes"][0]["operation"]["version"], "1.3.0");
    assert!(payload["intentDigest"].is_string());
    assert_eq!(payload["envelope"]["intentDigest"], payload["intentDigest"]);
    assert_nested_mutations_rejected(
        &run_schema("release-receipt"),
        &payload,
        &[
            ("/intentDigest", json!(false)),
            ("/envelope/intentDigest", json!({"digest": "a"})),
            ("/envelope/orchestrationRevision", json!(["revision"])),
            ("/envelope/releaseSourceRevision", json!(123)),
            (
                "/outcomes/0/operation/package",
                json!({"ecosystem": "cargo", "name": "widget-core"}),
            ),
            ("/outcomes/0/operation/version", json!({"raw": "1.3.0"})),
            ("/outcomes/0/operation/role/bindingDigest", json!(false)),
            ("/outcomes/0/outcome/kind", json!("pending")),
            ("/observations/0/observation/evidence/version", json!(130)),
            ("/observations/0/observation/evidence/checksum", json!(["digest"])),
            ("/observations/1/observation/evidence/peeled_commit", json!(false)),
        ],
    );
    let mut stdout = payload;
    stdout["command"] = json!("release execute");
    stdout["dryRun"] = json!(false);
    let schema = run_schema("release-receipt");
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(
        !validator.is_valid(&stdout),
        "durable receipt schema must reject stdout metadata"
    );
}

#[test]
fn decide_stdout_payload_matches_model_schema_and_verifies_unchanged() {
    let dir = common::setup_polyglot_git_repo();
    let repository = GitHubRepository::parse("example/widget").unwrap();
    let snapshot = ReleasePrSnapshotV2::new(
        repository.clone(),
        "main".into(),
        CommitSha::parse(&"a".repeat(40)).unwrap(),
        vec![ReleasePrPullRequestV2 {
            number: 7,
            head_repository: repository,
            head_branch: "feature/widget".into(),
            head_commit: CommitSha::parse(&"b".repeat(40)).unwrap(),
        }],
    )
    .unwrap();
    let snapshot_json = serde_json::to_string(&snapshot).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .current_dir(dir.path())
        .args([
            "--cwd",
            ".",
            "--format",
            "json",
            "release-pr",
            "decide",
            "--snapshot",
            &snapshot_json,
            "--repository",
            "example/widget",
            "--base-branch",
            "main",
            "--release-branch",
            "release/widget",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "decide failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout: Value = serde_json::from_slice(&output.stdout).unwrap();
    let mut payload = stdout.clone();
    assert_eq!(
        payload.as_object_mut().unwrap().remove("command"),
        Some(json!("release-pr decide"))
    );
    assert_eq!(payload.as_object_mut().unwrap().remove("dryRun"), Some(json!(false)));
    let schema = serde_json::to_value(schemars::schema_for!(ReleasePrDecisionV2)).unwrap();
    assert_nested_mutations_rejected(
        &schema,
        &payload,
        &[
            ("/config/repository", json!({"owner": "example", "name": "widget"})),
            ("/snapshot/openPullRequests/0/headCommit", json!(123)),
            ("/action/kind", json!("unknown")),
        ],
    );
    assert!(!jsonschema::validator_for(&schema).unwrap().is_valid(&stdout));
    let decision: ReleasePrDecisionV2 = serde_json::from_value(payload).unwrap();
    decision.verify_snapshot(&snapshot).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .current_dir(dir.path())
        .args([
            "--format",
            "json",
            "release-pr",
            "verify",
            "--decision",
            std::str::from_utf8(&output.stdout).unwrap(),
            "--snapshot",
            &snapshot_json,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "verify failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let verified: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(verified["command"], "release-pr verify");
    assert_eq!(verified["dryRun"], false);
    assert_eq!(verified["ok"], true);
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
