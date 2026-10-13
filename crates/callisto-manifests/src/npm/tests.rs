/// Tests exercise the write primitives directly rather than through a
/// command handler, so they mint a permit without a dry-run flag to
/// consult. Every non-test caller must go through
/// `ApplyPermit::granted_unless_dry_run`.
fn permit() -> callisto_model::ApplyPermit {
    callisto_model::ApplyPermit::force_for_tests()
}
use super::*;

#[test]
fn npm_package_name_reads_name() {
    let doc: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(r#"{"name":"my-pkg","version":"1.0.0"}"#).unwrap();
    assert_eq!(npm_package_name(&doc), Some("my-pkg"));
}

#[test]
fn napi_binary_name_falls_back_to_name_then_index() {
    let name = |json: &str| napi_binary_name(&serde_json::from_str(json).unwrap());
    assert_eq!(name(r#"{"napi":{"binaryName":"addon","name":"old"}}"#), "addon");
    assert_eq!(name(r#"{"napi":{"name":"old"}}"#), "old");
    assert_eq!(name(r#"{"napi":{}}"#), "index");
}

#[test]
fn npm_package_name_returns_none_when_absent() {
    let doc: serde_json::Map<String, serde_json::Value> = serde_json::from_str(r#"{"version":"1.0.0"}"#).unwrap();
    assert_eq!(npm_package_name(&doc), None);
}

/// Spec: `format_json_pretty` must propagate errors via Result rather
/// than unwrapping. The return type must be `Result<String, _>` so
/// callers can handle failures without a panic.
///
/// Before the fix the return type is `String`, so the `is_ok()` call
/// below fails to compile (String has no `is_ok` method).
#[test]
fn format_json_pretty_returns_result_not_panics() {
    let k = "version".to_string();
    let v = Value::String("1.0.0".to_string());
    let mut map = IndexMap::new();
    map.insert(&k, &v);
    let result: Result<String, _> = format_json_pretty(&map, "  ");
    assert!(result.is_ok());
    let json = result.unwrap();
    assert!(json.contains("\"version\""));
    assert!(json.contains("\"1.0.0\""));
}
use callisto_model::{ManifestDecl, ManifestFormat, ManifestRole};
use tempfile::tempdir;

#[test]
fn workspace_parser_preserves_every_suffix_and_catalog() {
    for suffix in [
        "",
        "*",
        "^",
        "~",
        "0.2.3",
        "^0.2.3",
        "@scope/lib@~0.2.3",
        "../lib",
        "^1 || ^2",
    ] {
        let raw = format!("workspace:{suffix}");
        let spec = parse_npm_spec_str(&raw, Some(WorkspaceKind::Pnpm));
        assert!(matches!(spec, DepSpec::WorkspaceProtocol(_)));
        assert_eq!(spec.render(), raw);
    }
    for raw in [
        "catalog:",
        "catalog:tools",
        "https://example.com/lib.tgz",
        "file:../lib",
        "git+https://example.com/lib",
    ] {
        assert_eq!(parse_npm_spec_str(raw, Some(WorkspaceKind::Pnpm)).render(), raw);
    }
    assert!(matches!(
        parse_npm_spec_str("catalog:", Some(WorkspaceKind::Pnpm)),
        DepSpec::Catalog(None)
    ));
    assert!(matches!(parse_npm_spec_str("workspace:^", None), DepSpec::Opaque(_)));
}

#[test]
fn ordinary_npm_disjunctions_remain_ranges_and_match_any_valid_arm() {
    let spec = parse_npm_spec_str("^1.0.0 || ^2.0.0", None);
    let DepSpec::Range(requirement, original) = spec else {
        panic!("valid npm disjunction should be understood as a range");
    };
    assert_eq!(original, "^1.0.0 || ^2.0.0");
    assert!(requirement
        .matches(&Version::parse("2.4.0", VersionGrammar::SemVer).unwrap())
        .unwrap());
    assert!(!requirement
        .matches(&Version::parse("3.0.0", VersionGrammar::SemVer).unwrap())
        .unwrap());
}

#[test]
fn workspace_round_trip_only_updates_safe_out_of_range_constraints() {
    let target = Version::parse("0.3.0", VersionGrammar::SemVer).unwrap();
    for (raw, expected) in [
        ("workspace:0.2.3", "workspace:0.3.0"),
        ("workspace:^0.2.3", "workspace:^0.3.0"),
        ("workspace:~0.2.3", "workspace:~0.3.0"),
        ("workspace:@scope/lib@^0.2.3", "workspace:@scope/lib@^0.3.0"),
    ] {
        let spec = parse_npm_spec_str(raw, Some(WorkspaceKind::Pnpm));
        assert_eq!(round_trip(&spec, &target).unwrap().render(), expected);
    }
    for raw in [
        "workspace:",
        "workspace:*",
        "workspace:^",
        "workspace:~",
        "workspace:../lib",
        "workspace:@scope/lib@*",
        "workspace:^0.3.0",
        "workspace:>=0.2.0",
        "workspace:>0.2.0",
        "workspace:<0.2.0",
        "workspace:^0.2.0 || ^0.4.0",
        "catalog:tools",
        "https://example.com/lib.tgz",
    ] {
        let spec = parse_npm_spec_str(raw, Some(WorkspaceKind::Pnpm));
        assert!(round_trip(&spec, &target).is_none(), "{raw}");
    }
    let snapshot = Version::parse("0.0.0-canary.1", VersionGrammar::SemVer).unwrap();
    let spec = parse_npm_spec_str("workspace:@scope/lib@^0.2.3", Some(WorkspaceKind::Pnpm));
    assert_eq!(
        round_trip(&spec, &snapshot).unwrap().render(),
        "workspace:@scope/lib@^0.0.0-canary.1"
    );
}

#[test]
fn workspace_alias_and_relative_targets_retain_declaration_keys() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("app")).unwrap();
    fs::create_dir_all(dir.path().join("lib")).unwrap();
    fs::write(
        dir.path().join("lib/package.json"),
        "\u{FEFF}{\"name\":\"@scope/lib\",\"version\":\"0.2.3\",\"private\":true}",
    )
    .unwrap();
    fs::write(dir.path().join("app/package.json"), r#"{
            "name":"app","version":"1.0.0","private":true,
            "dependencies":{"alias":"workspace:@scope/lib@^0.2.3","relative":"workspace:../lib","missing":"workspace:../absent"},
            "optionalDependencies":{"opt":"workspace:@scope/lib@~"},
            "peerDependencies":{"@scope/lib":"workspace:^"},
            "peerDependenciesMeta":{"@scope/lib":{"optional":true}}
        }"#).unwrap();
    let decl = ManifestDecl::new("app/package.json", ManifestRole::Canonical, ManifestFormat::PackageJson).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: Some(WorkspaceKind::Pnpm),
    };
    let manifest = PackageJson::open(&decl, &ctx).unwrap();
    let entries: Vec<_> = manifest.iter_dependencies().collect();
    for name in ["alias", "relative", "opt", "@scope/lib"] {
        let entry = entries.iter().find(|entry| entry.name == name).unwrap();
        assert_eq!(entry.package_name, "@scope/lib");
        assert!(matches!(entry.spec, DepSpec::WorkspaceProtocol(_)));
    }
    let missing = entries.iter().find(|entry| entry.name == "missing").unwrap();
    assert_eq!(missing.package_name, "workspace:../absent");
    assert_eq!(
        entries.iter().find(|entry| entry.name == "opt").unwrap().kind,
        DepKind::Optional
    );
    assert_eq!(
        entries.iter().find(|entry| entry.name == "@scope/lib").unwrap().kind,
        DepKind::Peer
    );
}

#[test]
fn workspace_rewrite_preserves_catalogs_and_manifest_format() {
    let dir = tempdir().unwrap();
    let content = "{\r\n\t\"name\": \"app\",\r\n\t\"version\": \"1.0.0\",\r\n\t\"dependencies\": {\r\n\t\t\"alias\": \"workspace:@scope/lib@^0.2.3\",\r\n\t\t\"catalogued\": \"catalog:tools\"\r\n\t}\r\n}\r\n";
    let mut manifest = open_manifest(&dir, content);
    let original = parse_npm_spec_str("workspace:@scope/lib@^0.2.3", Some(WorkspaceKind::Pnpm));
    let rewritten = round_trip(&original, &Version::parse("0.3.0", VersionGrammar::SemVer).unwrap()).unwrap();
    manifest
        .update_dependency_spec("alias", DepKind::Runtime, rewritten, &permit())
        .unwrap();
    manifest.persist(&permit()).unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("package.json")).unwrap(),
        content.replace("^0.2.3", "^0.3.0")
    );
}

#[test]
fn workspace_rewrite_leaves_override_and_resolution_sources_unchanged() {
    let dir = tempdir().unwrap();
    let mut manifest = open_manifest(
        &dir,
        r#"{
            "name":"app","version":"1.0.0",
            "dependencies":{"alias":"workspace:@scope/lib@^0.2.3"},
            "overrides":{"alias":"https://example.com/lib.tgz"},
            "resolutions":{"alias":"catalog:tools"}
        }"#,
    );
    let original = parse_npm_spec_str("workspace:@scope/lib@^0.2.3", Some(WorkspaceKind::Pnpm));
    let rewritten = round_trip(&original, &Version::parse("0.3.0", VersionGrammar::SemVer).unwrap()).unwrap();
    manifest
        .update_dependency_spec("alias", DepKind::Runtime, rewritten, &permit())
        .unwrap();
    manifest.persist(&permit()).unwrap();
    let doc: Value = serde_json::from_slice(&fs::read(dir.path().join("package.json")).unwrap()).unwrap();
    assert_eq!(doc["dependencies"]["alias"], "workspace:@scope/lib@^0.3.0");
    assert_eq!(doc["overrides"]["alias"], "https://example.com/lib.tgz");
    assert_eq!(doc["resolutions"]["alias"], "catalog:tools");
}

#[test]
fn pnpm_workspace_file_detects_protocol_before_a_lockfile_exists() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("pnpm-workspace.yaml"), "packages:\n  - packages/*\n").unwrap();
    assert_eq!(
        detect_npm_workspace_kind(dir.path()).unwrap(),
        Some(WorkspaceKind::Pnpm)
    );
}

/// Helper: write `content` to `<dir>/package.json` and open it as a
/// `PackageJson`. The caller must keep `dir` alive for the duration of the
/// test so that the tempdir is not cleaned up prematurely.
fn open_manifest(dir: &tempfile::TempDir, content: &str) -> PackageJson {
    let manifest_path = dir.path().join("package.json");
    fs::write(&manifest_path, content).unwrap();
    let decl = ManifestDecl::new("package.json", ManifestRole::Canonical, ManifestFormat::PackageJson).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };
    PackageJson::open(&decl, &ctx).unwrap()
}

#[test]
fn bin_names_read_string_and_object_bin_fields() {
    let dir = tempdir().unwrap();
    let library = open_manifest(&dir, r#"{"name":"lib","version":"1.0.0"}"#);
    assert!(library.bin_names().is_empty());
    let string = open_manifest(&dir, r#"{"name":"@scope/tool","version":"1.0.0","bin":"cli.js"}"#);
    assert_eq!(string.bin_names(), ["tool"]);
    let object = open_manifest(
        &dir,
        r#"{"name":"tools","version":"1.0.0","bin":{"alpha":"a.js","beta":"b.js"}}"#,
    );
    assert_eq!(object.bin_names(), ["alpha", "beta"]);
}

#[test]
fn write_version_does_not_touch_disk_until_persist_called() {
    let dir = tempdir().unwrap();
    let content = "{\n  \"name\": \"@myorg/pkg\",\n  \"version\": \"1.0.0\"\n}\n";
    let manifest_path = dir.path().join("package.json");
    let mut manifest = open_manifest(&dir, content);

    let new_ver = Version::parse("1.1.0", VersionGrammar::SemVer).unwrap();
    manifest.write_version(&new_ver, &permit()).unwrap();

    let unchanged = fs::read_to_string(&manifest_path).unwrap();
    assert_eq!(unchanged, content, "write_version alone must not write to disk");

    manifest.persist(&permit()).unwrap();
    let updated = fs::read_to_string(&manifest_path).unwrap();
    assert!(updated.contains("\"version\": \"1.1.0\""));
}

#[test]
fn update_optional_dependencies_does_not_touch_disk_until_persist_called() {
    let dir = tempdir().unwrap();
    let content = "{\n  \"name\": \"@myorg/pkg\",\n  \"version\": \"1.0.0\",\n  \"optionalDependencies\": {\n    \"fsevents\": \"1.0.0\"\n  }\n}\n";
    let manifest_path = dir.path().join("package.json");
    let mut manifest = open_manifest(&dir, content);

    let new_v = Version::parse("2.0.0", VersionGrammar::SemVer).unwrap();
    manifest
        .update_optional_dependencies(&[("fsevents".to_string(), new_v)], &permit())
        .unwrap();

    let unchanged = fs::read_to_string(&manifest_path).unwrap();
    assert_eq!(
        unchanged, content,
        "update_optional_dependencies alone must not write to disk"
    );

    manifest.persist(&permit()).unwrap();
    let updated = fs::read_to_string(&manifest_path).unwrap();
    assert!(updated.contains("\"fsevents\": \"2.0.0\""));
}

#[test]
fn update_dependency_spec_does_not_touch_disk_until_persist_called() {
    let dir = tempdir().unwrap();
    let content = "{\n  \"name\": \"@myorg/pkg\",\n  \"version\": \"1.0.0\",\n  \"dependencies\": {\n    \"lodash\": \"^4.17.0\"\n  }\n}\n";
    let manifest_path = dir.path().join("package.json");
    let mut manifest = open_manifest(&dir, content);

    manifest
        .update_dependency_spec(
            "lodash",
            DepKind::Runtime,
            DepSpec::Opaque("^5.0.0".to_string()),
            &permit(),
        )
        .unwrap();

    let unchanged = fs::read_to_string(&manifest_path).unwrap();
    assert_eq!(
        unchanged, content,
        "update_dependency_spec alone must not write to disk"
    );

    manifest.persist(&permit()).unwrap();
    let updated = fs::read_to_string(&manifest_path).unwrap();
    assert!(updated.contains("\"lodash\": \"^5.0.0\""));
}

#[test]
fn parses_package_json_and_updates_version() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("package.json");
    let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "express": "^4.18.0"
  }
}
"#;
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("package.json", ManifestRole::Canonical, ManifestFormat::PackageJson).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let mut manifest = PackageJson::open(&decl, &ctx).unwrap();
    assert_eq!(manifest.package_name().unwrap(), "@myorg/pkg");
    assert_eq!(manifest.current_version().unwrap().render(), "1.0.0");

    let new_ver = Version::parse("1.1.0", VersionGrammar::SemVer).unwrap();
    manifest.write_version(&new_ver, &permit()).unwrap();
    manifest.persist(&permit()).unwrap();

    let updated_content = fs::read_to_string(&manifest_path).unwrap();
    assert!(updated_content.contains("\"version\": \"1.1.0\""));
}

#[test]
fn detects_workspace_kind() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();
    assert_eq!(
        detect_npm_workspace_kind(dir.path()).unwrap(),
        Some(WorkspaceKind::Pnpm)
    );
}

#[test]
fn parses_package_json_with_utf8_bom() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("package.json");
    let content = "\u{FEFF}{\n  \"name\": \"bom-pkg\",\n  \"version\": \"1.0.0\"\n}\n";
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("package.json", ManifestRole::Canonical, ManifestFormat::PackageJson).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let manifest = PackageJson::open(&decl, &ctx).unwrap();
    assert_eq!(manifest.package_name().unwrap(), "bom-pkg");
    assert_eq!(manifest.current_version().unwrap().render(), "1.0.0");
}

#[test]
fn update_dependency_spec_does_not_mutate_overrides_when_primary_section_missing() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("package.json");
    let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "express": "^4.18.0"
  },
  "overrides": {
    "lodash": "^4.17.0"
  }
}
"#;
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("package.json", ManifestRole::Canonical, ManifestFormat::PackageJson).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let mut manifest = PackageJson::open(&decl, &ctx).unwrap();

    let before = manifest.doc.get("overrides").cloned();

    // "lodash" exists in overrides but NOT in the "dependencies" section,
    // so the primary-section update must fail.
    let result = manifest.update_dependency_spec(
        "lodash",
        DepKind::Runtime,
        DepSpec::Opaque("^5.0.0".to_string()),
        &permit(),
    );

    assert!(result.is_err());

    let after = manifest.doc.get("overrides").cloned();
    assert_eq!(
        before, after,
        "overrides table must remain unchanged when the primary section update fails"
    );

    // The file on disk must also remain untouched.
    let on_disk = fs::read_to_string(&manifest_path).unwrap();
    assert!(on_disk.contains("\"lodash\": \"^4.17.0\""));
}

// --- Happy-path co-mutation of overrides/resolutions ---

#[test]
fn update_dependency_spec_co_mutates_overrides() {
    let dir = tempdir().unwrap();
    let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "lodash": "^4.17.0"
  },
  "overrides": {
    "lodash": "^4.17.0"
  }
}
"#;
    let mut manifest = open_manifest(&dir, content);

    manifest
        .update_dependency_spec(
            "lodash",
            DepKind::Runtime,
            DepSpec::Opaque("^5.0.0".to_string()),
            &permit(),
        )
        .unwrap();

    let deps = manifest.doc.get("dependencies").and_then(|v| v.as_object()).unwrap();
    assert_eq!(
        deps.get("lodash").and_then(|v| v.as_str()),
        Some("^5.0.0"),
        "dependencies must reflect the new version"
    );

    let overrides = manifest.doc.get("overrides").and_then(|v| v.as_object()).unwrap();
    assert_eq!(
        overrides.get("lodash").and_then(|v| v.as_str()),
        Some("^5.0.0"),
        "overrides must be co-mutated when the dep exists in the primary section"
    );
}

#[test]
fn update_dependency_spec_co_mutates_resolutions() {
    let dir = tempdir().unwrap();
    let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "lodash": "^4.17.0"
  },
  "resolutions": {
    "lodash": "^4.17.0"
  }
}
"#;
    let mut manifest = open_manifest(&dir, content);

    manifest
        .update_dependency_spec(
            "lodash",
            DepKind::Runtime,
            DepSpec::Opaque("^5.0.0".to_string()),
            &permit(),
        )
        .unwrap();

    let deps = manifest.doc.get("dependencies").and_then(|v| v.as_object()).unwrap();
    assert_eq!(
        deps.get("lodash").and_then(|v| v.as_str()),
        Some("^5.0.0"),
        "dependencies must reflect the new version"
    );

    let resolutions = manifest.doc.get("resolutions").and_then(|v| v.as_object()).unwrap();
    assert_eq!(
        resolutions.get("lodash").and_then(|v| v.as_str()),
        Some("^5.0.0"),
        "resolutions must be co-mutated when the dep exists in the primary section"
    );
}

#[test]
fn update_dependency_spec_co_mutates_overrides_and_resolutions() {
    let dir = tempdir().unwrap();
    let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "lodash": "^4.17.0"
  },
  "overrides": {
    "lodash": "^4.17.0"
  },
  "resolutions": {
    "lodash": "^4.17.0"
  }
}
"#;
    let mut manifest = open_manifest(&dir, content);

    manifest
        .update_dependency_spec(
            "lodash",
            DepKind::Runtime,
            DepSpec::Opaque("^5.0.0".to_string()),
            &permit(),
        )
        .unwrap();

    let deps = manifest.doc.get("dependencies").and_then(|v| v.as_object()).unwrap();
    assert_eq!(
        deps.get("lodash").and_then(|v| v.as_str()),
        Some("^5.0.0"),
        "dependencies must reflect the new version"
    );

    let overrides = manifest.doc.get("overrides").and_then(|v| v.as_object()).unwrap();
    assert_eq!(
        overrides.get("lodash").and_then(|v| v.as_str()),
        Some("^5.0.0"),
        "overrides must be co-mutated"
    );

    let resolutions = manifest.doc.get("resolutions").and_then(|v| v.as_object()).unwrap();
    assert_eq!(
        resolutions.get("lodash").and_then(|v| v.as_str()),
        Some("^5.0.0"),
        "resolutions must be co-mutated"
    );
}

/// Co-mutation is keyed by name: updating "express" must not touch the
/// "lodash" entry that happens to live in overrides.
#[test]
fn update_dependency_spec_co_mutation_is_name_scoped() {
    let dir = tempdir().unwrap();
    let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "express": "^4.18.0"
  },
  "overrides": {
    "lodash": "^4.17.0"
  }
}
"#;
    let mut manifest = open_manifest(&dir, content);

    manifest
        .update_dependency_spec(
            "express",
            DepKind::Runtime,
            DepSpec::Opaque("^5.0.0".to_string()),
            &permit(),
        )
        .unwrap();

    let deps = manifest.doc.get("dependencies").and_then(|v| v.as_object()).unwrap();
    assert_eq!(
        deps.get("express").and_then(|v| v.as_str()),
        Some("^5.0.0"),
        "express in dependencies must be updated"
    );

    let overrides = manifest.doc.get("overrides").and_then(|v| v.as_object()).unwrap();
    assert_eq!(
        overrides.get("lodash").and_then(|v| v.as_str()),
        Some("^4.17.0"),
        "lodash in overrides must be untouched (different name)"
    );
}

// --- Resolutions missing from validate-before-mutate regression ---

#[test]
fn update_dependency_spec_does_not_mutate_resolutions_when_primary_section_missing() {
    let dir = tempdir().unwrap();
    let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "express": "^4.18.0"
  },
  "resolutions": {
    "lodash": "^4.17.0"
  }
}
"#;
    let mut manifest = open_manifest(&dir, content);

    let before = manifest.doc.get("resolutions").cloned();

    let result = manifest.update_dependency_spec(
        "lodash",
        DepKind::Runtime,
        DepSpec::Opaque("^5.0.0".to_string()),
        &permit(),
    );

    assert!(
        result.is_err(),
        "update must fail when dep is absent from primary section"
    );

    let after = manifest.doc.get("resolutions").cloned();
    assert_eq!(
        before, after,
        "resolutions table must remain unchanged when the primary section update fails"
    );
}

/// Combined case: package in both overrides AND resolutions but not in
/// the primary section. Both tables must be untouched on failure.
#[test]
fn update_dependency_spec_does_not_mutate_overrides_or_resolutions_when_primary_section_missing() {
    let dir = tempdir().unwrap();
    let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "express": "^4.18.0"
  },
  "overrides": {
    "lodash": "^4.17.0"
  },
  "resolutions": {
    "lodash": "^4.17.0"
  }
}
"#;
    let mut manifest = open_manifest(&dir, content);

    let before_overrides = manifest.doc.get("overrides").cloned();
    let before_resolutions = manifest.doc.get("resolutions").cloned();

    let result = manifest.update_dependency_spec(
        "lodash",
        DepKind::Runtime,
        DepSpec::Opaque("^5.0.0".to_string()),
        &permit(),
    );

    assert!(
        result.is_err(),
        "update must fail when dep is absent from primary section"
    );

    let after_overrides = manifest.doc.get("overrides").cloned();
    let after_resolutions = manifest.doc.get("resolutions").cloned();

    assert_eq!(
        before_overrides, after_overrides,
        "overrides table must remain unchanged when the primary section update fails"
    );
    assert_eq!(
        before_resolutions, after_resolutions,
        "resolutions table must remain unchanged when the primary section update fails"
    );
}

// --- Nested overrides object must not be replaced with a string ---

/// When `overrides["foo"]` is a nested object (e.g. `{ "bar": "^1.0.0" }`),
/// bumping `foo` must leave that object intact. The nested form expresses a
/// package-level constraint that is not a simple version pin and must not be
/// overwritten.
#[test]
fn update_dependency_spec_does_not_overwrite_nested_overrides_object() {
    let dir = tempdir().unwrap();
    let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "dependencies": {
    "foo": "^1.0.0"
  },
  "overrides": {
    "foo": {
      "bar": "^1.0.0"
    }
  }
}
"#;
    let mut manifest = open_manifest(&dir, content);

    manifest
        .update_dependency_spec(
            "foo",
            DepKind::Runtime,
            DepSpec::Opaque("^2.0.0".to_string()),
            &permit(),
        )
        .unwrap();

    let overrides_foo = manifest
        .doc
        .get("overrides")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("foo"))
        .expect("overrides.foo must still exist");

    assert!(
        overrides_foo.is_object(),
        "overrides.foo must remain an object, but got: {overrides_foo:?}"
    );
}

// --- DepKind::Dev validate-before-mutate path ---

#[test]
fn update_dependency_spec_does_not_mutate_overrides_when_dev_dep_missing() {
    let dir = tempdir().unwrap();
    let content = r#"{
  "name": "@myorg/pkg",
  "version": "1.0.0",
  "devDependencies": {
    "typescript": "^5.0.0"
  },
  "overrides": {
    "lodash": "^4.17.0"
  }
}
"#;
    let mut manifest = open_manifest(&dir, content);

    let before = manifest.doc.get("overrides").cloned();

    let result =
        manifest.update_dependency_spec("lodash", DepKind::Dev, DepSpec::Opaque("^5.0.0".to_string()), &permit());

    assert!(
        result.is_err(),
        "update must fail when dep is absent from devDependencies"
    );

    let after = manifest.doc.get("overrides").cloned();
    assert_eq!(
        before, after,
        "overrides table must remain unchanged when the dev dep update fails"
    );
}

/// A package.json that contains a top-level `"workspaces"` array must be
/// detected as an NPM workspace root by `detect_npm_workspace_kind`, even
/// when no lock-file is present on disk (the lock-file heuristic alone is
/// not sufficient for projects that have not yet run `npm install`).
#[test]
fn detects_workspace_kind_from_workspaces_field_in_package_json() {
    let dir = tempdir().unwrap();
    let pkg_json = dir.path().join("package.json");
    let content = r#"{
  "name": "my-monorepo",
  "version": "1.0.0",
  "workspaces": [
    "packages/*"
  ]
}
"#;
    fs::write(&pkg_json, content).unwrap();

    // No lock-file on disk, only the "workspaces" field in package.json.
    let kind = detect_npm_workspace_kind(dir.path()).unwrap();
    assert_eq!(
        kind,
        Some(WorkspaceKind::Npm),
        "workspaces field in package.json must imply WorkspaceKind::Npm"
    );
}

/// A scoped package name like `"@scope/name"` must round-trip through
/// `package_name()` unchanged. Scoped names contain a `/` character that
/// could be mistaken for a path separator; this test verifies it is
/// treated as a literal package-name character.
#[test]
fn scoped_package_name_is_parsed_correctly() {
    let dir = tempdir().unwrap();
    let content = r#"{
  "name": "@my-scope/awesome-pkg",
  "version": "2.3.4"
}
"#;
    let manifest = open_manifest(&dir, content);
    assert_eq!(
        manifest.package_name().unwrap(),
        "@my-scope/awesome-pkg",
        "scoped package name must be returned verbatim"
    );
}

/// A `package.json` that is missing the `"version"` field must make
/// `current_version()` return `Err(ManifestError::MissingField)` rather
/// than panicking or returning a default value.
#[test]
fn missing_version_field_returns_missing_field_error_not_panic() {
    let dir = tempdir().unwrap();
    let content = r#"{
  "name": "no-version-pkg",
  "description": "This package intentionally omits the version field"
}
"#;
    let manifest = open_manifest(&dir, content);
    let result = manifest.current_version();
    assert!(
        matches!(result, Err(ManifestError::MissingField { field: "version", .. })),
        "missing version field must return MissingField error, got: {result:?}"
    );
}

#[test]
fn npm_declares_private_reads_the_private_field() {
    assert!(npm_declares_private(r#"{"name":"root","private":true}"#));
    assert!(!npm_declares_private(r#"{"name":"root","private":false}"#));
    assert!(!npm_declares_private(r#"{"name":"root"}"#));
    assert!(!npm_declares_private("not json"));
}

/// Tab-indented package.json files must have their indentation preserved after a
/// version write. This catches regressions where the CST editor re-serializes with
/// two-space indentation instead of the original tab characters.
#[test]
fn tab_indentation_is_preserved_after_version_write() {
    use callisto_model::WorkspaceKind;
    let dir = tempdir().unwrap();
    let path = dir.path().join("package.json");
    let content = "{\n\t\"name\": \"tab-app\",\n\t\"version\": \"1.0.0\"\n}\n";
    fs::write(&path, content).unwrap();

    let decl = ManifestDecl::new("package.json", ManifestRole::Canonical, ManifestFormat::PackageJson).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: Some(WorkspaceKind::Pnpm),
    };

    let mut pj = PackageJson::open(&decl, &ctx).unwrap();
    pj.write_version(
        &callisto_model::Version::parse("1.1.0", callisto_model::VersionGrammar::SemVer).unwrap(),
        &permit(),
    )
    .unwrap();
    pj.persist(&permit()).unwrap();

    let updated = fs::read_to_string(&path).unwrap();
    assert!(updated.contains("\"version\": \"1.1.0\""));
    assert!(
        updated.contains("\t\"version\""),
        "tab indentation must be preserved after version write"
    );
}

/// Regression lock for the `npm.rs` half of F4's shared-fingerprint
/// migration: a `package.json` with CRLF line endings must keep them
/// across a `write_version` + `persist` round trip. `npm.rs` previously
/// had its own private `LineEnding` detection/reapply logic (now
/// migrated onto `crate::common::FormatFingerprint`) but no test ever
/// exercised the CRLF branch directly.
#[test]
fn preserves_crlf_line_endings_on_write() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("package.json");
    let content = "{\r\n  \"name\": \"crlf-app\",\r\n  \"version\": \"1.0.0\"\r\n}\r\n";
    fs::write(&path, content).unwrap();

    let decl = ManifestDecl::new("package.json", ManifestRole::Canonical, ManifestFormat::PackageJson).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let mut pj = PackageJson::open(&decl, &ctx).unwrap();
    pj.write_version(
        &callisto_model::Version::parse("1.1.0", callisto_model::VersionGrammar::SemVer).unwrap(),
        &permit(),
    )
    .unwrap();
    pj.persist(&permit()).unwrap();

    let updated_bytes = fs::read(&path).unwrap();
    let updated = String::from_utf8(updated_bytes).unwrap();

    assert!(
        !updated.starts_with('\u{FEFF}'),
        "expected no BOM to be introduced, got:\n{updated:?}"
    );
    assert!(
        updated.contains("\r\n"),
        "expected CRLF line endings to survive write, got:\n{updated:?}"
    );
    assert!(
        !updated.replace("\r\n", "").contains('\n'),
        "expected no bare LF line endings to remain, got:\n{updated:?}"
    );
    assert!(updated.contains("\"version\": \"1.1.0\""));
}

// --- round_trip tests ---------------------------------------------------

fn make_npm_spec(raw: &str) -> DepSpec {
    let req = VersionReq::parse(raw, Ecosystem::Npm).unwrap();
    DepSpec::Range(req, raw.to_string())
}

fn npm_version(s: &str) -> Version {
    Version::parse(s, VersionGrammar::SemVer).unwrap()
}

fn raw_of(spec: Option<DepSpec>) -> Option<String> {
    match spec {
        Some(DepSpec::Range(_, raw)) => Some(raw),
        _ => None,
    }
}

#[test]
fn round_trip_exact_spec_rewrites_to_target_version() {
    let spec = DepSpec::Exact(npm_version("1.0.0"));
    let target = npm_version("2.3.4");
    match round_trip(&spec, &target) {
        Some(DepSpec::Exact(v)) => assert_eq!(v.render(), "2.3.4"),
        other => panic!("expected Some(Exact(2.3.4)), got {other:?}"),
    }
}

#[test]
fn round_trip_caret_range_rewrites_to_target_version() {
    let spec = make_npm_spec("^1.0.0");
    let target = npm_version("1.5.2");
    assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some("^1.5.2"));
}

#[test]
fn round_trip_tilde_range_rewrites_to_target_version() {
    let spec = make_npm_spec("~1.0.0");
    let target = npm_version("1.0.9");
    assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some("~1.0.9"));
}

#[test]
fn round_trip_gte_range_rewrites_to_target_version() {
    let spec = make_npm_spec(">=1.0.0");
    let target = npm_version("2.5.1");
    assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some(">=2.5.1"));
}

#[test]
fn round_trip_exact_operator_range_rewrites_to_target_version() {
    let spec = make_npm_spec("=1.0.0");
    let target = npm_version("1.2.3");
    assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some("=1.2.3"));
}

#[test]
fn round_trip_preserves_two_part_precision() {
    let spec = make_npm_spec("^1.0");
    let target = npm_version("1.5.2");
    assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some("^1.5"));
}

#[test]
fn round_trip_preserves_one_part_precision() {
    let spec = make_npm_spec("^1");
    let target = npm_version("2.5.2");
    assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some("^2"));
}

/// A prerelease target version (e.g. publishing `2.0.0-beta.1`) must be
/// rendered in full, not truncated to `major.minor.patch` -- dropping the
/// prerelease suffix would produce a range that does not actually match
/// the version it claims to pin to (`^2.0.0` excludes `2.0.0-beta.1`
/// under semver precedence rules).
#[test]
fn round_trip_prerelease_target_is_rendered_in_full() {
    let spec = make_npm_spec("^1.0.0");
    let target = npm_version("2.0.0-beta.1");
    assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some("^2.0.0-beta.1"));
}

#[test]
fn round_trip_non_range_variants_return_none() {
    let target = npm_version("2.0.0");
    assert!(round_trip(&DepSpec::Opaque("weird".to_string()), &target).is_none());
    assert!(round_trip(&DepSpec::Workspace(WorkspaceKind::Npm), &target).is_none());
    assert!(round_trip(&DepSpec::Catalog(None), &target).is_none());
    assert!(round_trip(&DepSpec::CargoBare(npm_version("1.0.0")), &target).is_none());
}

/// A clause with a prerelease suffix (e.g. `^1.0.0-beta.1`) is declined
/// rather than rewritten -- `render_at_precision` only knows how to
/// truncate a plain `major.minor.patch` clause, so a prerelease-qualified
/// original clause safely falls back to `RewriteOutcome::LeftAlone`.
#[test]
fn round_trip_prerelease_clause_returns_none() {
    let spec = make_npm_spec("^1.0.0-beta.1");
    let target = npm_version("2.0.0");
    assert!(round_trip(&spec, &target).is_none());
}

/// A wildcard clause (`1.x`, `1.X`, `^1.x`) is declined rather than
/// rewritten -- these are not simple `{operator}{version}` clauses and
/// `render_at_precision` has no wildcard-preserving behavior.
#[test]
fn round_trip_wildcard_clause_returns_none() {
    let target = npm_version("2.5.0");
    assert!(round_trip(&make_npm_spec("1.x"), &target).is_none());
    assert!(round_trip(&make_npm_spec("1.X"), &target).is_none());
    assert!(round_trip(&make_npm_spec("^1.x"), &target).is_none());
}

/// A comma-separated compound range (`>=1.0.0, <2.0.0`) parses
/// successfully -- npm and Cargo share the same underlying semver
/// requirement grammar in this codebase (`semver::VersionReq`), so this
/// atypical-for-npm-but-syntactically-valid form does reach this
/// function -- but it is declined rather than rewritten. Unlike
/// `cargo::round_trip`, `npm::round_trip` has no per-clause compound-range
/// handling, so this documents current, intentional (safe-fallback)
/// behavior: the caller falls back to `RewriteOutcome::LeftAlone` with a
/// diagnostic instead of guessing at a rewrite.
#[test]
fn round_trip_comma_compound_range_returns_none() {
    let spec = make_npm_spec(">=1.0.0, <2.0.0");
    let target = npm_version("1.5.0");
    assert!(round_trip(&spec, &target).is_none());
}

#[test]
fn persist_is_exposed_as_a_manifest_trait_method() {
    let dir = tempdir().unwrap();
    let content = "{\n  \"name\": \"@myorg/pkg\",\n  \"version\": \"1.0.0\"\n}\n";
    let manifest_path = dir.path().join("package.json");
    let mut manifest = open_manifest(&dir, content);

    <PackageJson as Manifest>::persist(&mut manifest, &permit()).unwrap();

    let after = fs::read_to_string(&manifest_path).unwrap();
    assert_eq!(
        after, content,
        "persist() with no prior mutation must reproduce the file unchanged"
    );
}

// --- npm_role tests (F23: moved from callisto-graph::walk's detect_npm_role) ---

/// F23 regression: `npm_role()` must derive the platform role from the
/// `Manifest` handle's already-parsed document, not a fresh disk read.
/// Proven by deleting the underlying file after `open()` and confirming
/// `npm_role()` still reports the correct role -- a re-implementation
/// that shelled out to a fresh `fs::read` internally would instead
/// silently report `None` here.
#[test]
fn npm_role_does_not_re_read_the_manifest_from_disk() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("package.json");
    fs::write(
        &manifest_path,
        r#"{"name":"@scope/my-lib-linux-x64-gnu","version":"1.0.0","os":["linux"],"cpu":["x64"]}"#,
    )
    .unwrap();
    let manifest = open_manifest(&dir, &fs::read_to_string(&manifest_path).unwrap());

    fs::remove_file(&manifest_path).unwrap();

    let role = manifest.npm_role();
    assert_eq!(
        role,
        Some(NpmRole::Platform {
            platform: "linux".to_string(),
            arch: "x64".to_string(),
            abi: Some("gnu".to_string()),
        }),
        "npm_role() must still resolve correctly from the in-memory document \
             after the underlying file is gone, proving it does no second read"
    );
}

#[test]
fn npm_role_is_none_for_a_plain_non_platform_package() {
    let dir = tempdir().unwrap();
    let manifest = open_manifest(&dir, r#"{"name":"plain-pkg","version":"1.0.0"}"#);
    assert_eq!(manifest.npm_role(), None);
}

#[test]
fn npm_role_from_doc_infers_linux_gnu_abi_from_package_name_suffix() {
    let doc: Map<String, Value> = serde_json::from_str(
        r#"{"name":"@scope/my-lib-linux-x64-gnu","version":"1.0.0","os":["linux"],"cpu":["x64"]}"#,
    )
    .unwrap();
    assert_eq!(
        npm_role_from_doc(&doc),
        Some(NpmRole::Platform {
            platform: "linux".to_string(),
            arch: "x64".to_string(),
            abi: Some("gnu".to_string()),
        })
    );
}

#[test]
fn npm_role_from_doc_infers_linux_musl_abi_from_package_name_suffix() {
    let doc: Map<String, Value> = serde_json::from_str(
        r#"{"name":"@scope/my-lib-linux-arm64-musl","version":"1.0.0","os":["linux"],"cpu":["arm64"]}"#,
    )
    .unwrap();
    assert_eq!(
        npm_role_from_doc(&doc),
        Some(NpmRole::Platform {
            platform: "linux".to_string(),
            arch: "arm64".to_string(),
            abi: Some("musl".to_string()),
        })
    );
}

#[test]
fn npm_role_from_doc_infers_linux_gnueabihf_abi_from_package_name_suffix() {
    let doc: Map<String, Value> = serde_json::from_str(
        r#"{"name":"@scope/my-lib-linux-arm-gnueabihf","version":"1.0.0","os":["linux"],"cpu":["arm"]}"#,
    )
    .unwrap();
    assert_eq!(
        npm_role_from_doc(&doc),
        Some(NpmRole::Platform {
            platform: "linux".to_string(),
            arch: "arm".to_string(),
            abi: Some("gnueabihf".to_string()),
        })
    );
}

/// Non-Linux platforms have no ABI concept -- must not be affected by the
/// name-suffix inference at all.
#[test]
fn npm_role_from_doc_does_not_infer_abi_for_non_linux_platforms() {
    let doc: Map<String, Value> = serde_json::from_str(
        r#"{"name":"@scope/my-lib-darwin-arm64-gnu","version":"1.0.0","os":["darwin"],"cpu":["arm64"]}"#,
    )
    .unwrap();
    assert_eq!(
        npm_role_from_doc(&doc),
        Some(NpmRole::Platform {
            platform: "darwin".to_string(),
            arch: "arm64".to_string(),
            abi: None,
        })
    );
}

/// A Linux platform package whose name has no recognized ABI suffix at
/// all stays `abi: None` -- this infers when it can, it doesn't invent an
/// ABI that isn't actually signaled anywhere.
#[test]
fn npm_role_from_doc_leaves_abi_none_when_linux_name_has_no_recognized_suffix() {
    let doc: Map<String, Value> =
        serde_json::from_str(r#"{"name":"@scope/my-lib-linux-x64","version":"1.0.0","os":["linux"],"cpu":["x64"]}"#)
            .unwrap();
    assert_eq!(
        npm_role_from_doc(&doc),
        Some(NpmRole::Platform {
            platform: "linux".to_string(),
            arch: "x64".to_string(),
            abi: None,
        })
    );
}

#[test]
fn npm_role_from_doc_returns_none_when_os_or_cpu_missing() {
    let doc: Map<String, Value> = serde_json::from_str(r#"{"name":"plain-pkg","version":"1.0.0"}"#).unwrap();
    assert_eq!(npm_role_from_doc(&doc), None);
}
