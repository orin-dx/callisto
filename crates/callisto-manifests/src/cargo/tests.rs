/// Tests exercise the write primitives directly rather than through a
/// command handler, so they mint a permit without a dry-run flag to
/// consult. Every non-test caller must go through
/// `ApplyPermit::granted_unless_dry_run`.
fn permit() -> callisto_model::ApplyPermit {
    callisto_model::ApplyPermit::force_for_tests()
}
use super::*;
use callisto_model::{ManifestDecl, ManifestFormat, ManifestRole};
use tempfile::tempdir;

#[test]
fn cargo_package_name_reads_package_name() {
    let doc: toml_edit::DocumentMut = "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n".parse().unwrap();
    assert_eq!(cargo_package_name(&doc), Some("my-crate"));
}

fn open_cargo(dir: &std::path::Path, content: &str) -> CargoToml {
    fs::write(dir.join("Cargo.toml"), content).unwrap();
    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir,
        cargo_workspace: None,
        npm_workspace_kind: None,
    };
    CargoToml::open(&decl, &ctx).unwrap()
}

#[test]
fn bin_names_include_explicit_and_automatic_targets() {
    let package = "[package]\nname = \"app\"\nversion = \"1.0.0\"\n";
    let dir = tempdir().unwrap();
    assert!(open_cargo(dir.path(), package).bin_names().is_empty(), "library-only");

    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    assert_eq!(open_cargo(dir.path(), package).bin_names(), ["app"]);

    let bins = format!("{package}\n[[bin]]\nname = \"first\"\n\n[[bin]]\nname = \"second\"\n");
    assert_eq!(open_cargo(dir.path(), &bins).bin_names(), ["app", "first", "second"]);
}

#[test]
fn bin_names_discover_src_bin_files_and_nested_main_files() {
    let dir = tempdir().unwrap();
    let package = "[package]\nname = \"app\"\nversion = \"1.0.0\"\n";
    fs::create_dir_all(dir.path().join("src/bin/tool")).unwrap();
    fs::write(dir.path().join("src/bin/serve.rs"), "fn main() {}\n").unwrap();
    fs::write(dir.path().join("src/bin/tool/main.rs"), "fn main() {}\n").unwrap();
    assert_eq!(open_cargo(dir.path(), package).bin_names(), ["serve", "tool"]);
}

#[test]
fn autobins_false_keeps_only_declared_binary_targets() {
    let dir = tempdir().unwrap();
    let package = "[package]\nname = \"app\"\nversion = \"1.0.0\"\nautobins = false\n";
    fs::create_dir_all(dir.path().join("src/bin")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(dir.path().join("src/bin/auto.rs"), "fn main() {}\n").unwrap();
    assert!(open_cargo(dir.path(), package).bin_names().is_empty());
    let declared = format!("{package}\n[[bin]]\nname = \"declared\"\npath = \"tools/entry.rs\"\n");
    assert_eq!(open_cargo(dir.path(), &declared).bin_names(), ["declared"]);
}

#[test]
fn duplicate_explicit_and_automatic_binary_names_are_deduplicated() {
    let dir = tempdir().unwrap();
    let package = "[package]\nname = \"app\"\nversion = \"1.0.0\"\n";
    fs::create_dir_all(dir.path().join("src/bin")).unwrap();
    fs::write(dir.path().join("src/bin/app.rs"), "fn main() {}\n").unwrap();
    let declared = format!("{package}\n[[bin]]\nname = \"app\"\npath = \"src/bin/app.rs\"\n");
    assert_eq!(open_cargo(dir.path(), &declared).bin_names(), ["app"]);
}

#[test]
fn napi_lib_name_needs_a_cdylib_depending_on_napi() {
    let dir = tempdir().unwrap();
    let package = "[package]\nname = \"my-binding\"\nversion = \"1.0.0\"\n";
    let cdylib = "[lib]\ncrate-type = [\"cdylib\"]\n";
    let napi = "[dependencies]\nnapi = \"3\"\n";
    let cases = [
        (format!("{package}{cdylib}{napi}"), Some("my_binding")),
        (
            format!("{package}[lib]\nname = \"addon\"\ncrate-type = [\"cdylib\"]\n{napi}"),
            Some("addon"),
        ),
        (format!("{package}{napi}"), None),
        (format!("{package}{cdylib}"), None),
    ];
    for (content, expected) in cases {
        assert_eq!(
            open_cargo(dir.path(), &content).napi_lib_name().as_deref(),
            expected,
            "{content}"
        );
    }
}

#[test]
fn cargo_package_name_returns_none_when_absent() {
    let doc: toml_edit::DocumentMut = "[workspace]\nmembers = [\"crates/*\"]\n".parse().unwrap();
    assert_eq!(cargo_package_name(&doc), None);
}

#[test]
fn workspace_root_manifest_path_normalizes_to_bare_cargo_toml() {
    let content =
        "[workspace]\nmembers = [\".\"]\n\n[package]\nname = \"root-pkg\"\nversion = \"1.0.0\"\nedition = \"2021\"\n";

    // Absolute root case: exercises WorkspaceCargoResolver::load's
    // is_absolute()==true branch (root_manifest_path.file_name()).
    let dir = tempdir().unwrap();
    let abs_root_cargo = dir.path().join("Cargo.toml");
    fs::write(&abs_root_cargo, content).unwrap();
    let abs_inheritance = WorkspaceCargoResolver::load(&abs_root_cargo)
        .unwrap()
        .inheritance()
        .unwrap();
    assert_eq!(abs_inheritance.root_manifest, PathBuf::from("Cargo.toml"));

    // Relative-input case: WorkspaceCargoResolver::load's is_absolute()==false
    // branch delegates directly to workspace_relative(root_manifest_path) with
    // no other transformation -- asserted directly here (rather than via a
    // cwd-swapping call to load()) since workspace_relative is the exact,
    // pure, cwd-independent function that branch calls.
    assert_eq!(
        callisto_model::workspace_relative(Path::new("Cargo.toml")).unwrap(),
        PathBuf::from("Cargo.toml")
    );

    // The other side of classify_manifest_writes' comparison: ManifestDecl::new's
    // own workspace_relative normalization of the workspace root's own canonical
    // manifest path.
    let decl = ManifestDecl::new(
        PathBuf::from(".").join("Cargo.toml"),
        ManifestRole::Canonical,
        ManifestFormat::CargoToml,
    )
    .unwrap();
    assert_eq!(decl.path, PathBuf::from("Cargo.toml"));
    assert_eq!(
            decl.path, abs_inheritance.root_manifest,
            "a root package's own canonical manifest path and WorkspaceCargoResolver's root_manifest must normalize identically for classify_manifest_writes' plain PathBuf equality to detect them as the same physical file"
        );
}

#[test]
fn persist_is_exposed_as_a_manifest_trait_method() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let content = "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n";
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    <CargoToml as Manifest>::persist(&mut manifest, &permit()).unwrap();

    let after = fs::read_to_string(&manifest_path).unwrap();
    assert_eq!(
        after, content,
        "persist() with no prior mutation must reproduce the file unchanged"
    );
}

#[test]
fn parses_cargo_toml_and_updates_version() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let content = r#"[package]
name = "my-crate"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = "1.0"
"#;
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    assert_eq!(manifest.package_name().unwrap(), "my-crate");
    assert_eq!(manifest.current_version().unwrap().render(), "0.1.0");

    let new_ver = Version::parse("0.2.0", VersionGrammar::SemVer).unwrap();
    manifest.write_version(&new_ver, &permit()).unwrap();
    manifest.persist(&permit()).unwrap();

    let updated_content = fs::read_to_string(&manifest_path).unwrap();
    assert!(updated_content.contains("version = \"0.2.0\""));
}

#[test]
fn write_version_does_not_touch_disk_until_persist_called() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let content = "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n";
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    let new_ver = Version::parse("0.2.0", VersionGrammar::SemVer).unwrap();
    manifest.write_version(&new_ver, &permit()).unwrap();

    let unchanged = fs::read_to_string(&manifest_path).unwrap();
    assert_eq!(unchanged, content, "write_version alone must not write to disk");

    manifest.persist(&permit()).unwrap();
    let updated = fs::read_to_string(&manifest_path).unwrap();
    assert!(updated.contains("version = \"0.2.0\""));
}

#[test]
fn handles_utf8_bom_cargo_toml() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    fs::write(&manifest_path, callisto_fixtures::corpus::cargo_toml_bom_sample()).unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let manifest = CargoToml::open(&decl, &ctx).unwrap();
    assert_eq!(manifest.package_name().unwrap(), "bom-crate");
    assert_eq!(manifest.current_version().unwrap().render(), "1.0.0");
}

#[test]
fn preserves_bom_on_write_round_trip() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    fs::write(&manifest_path, callisto_fixtures::corpus::cargo_toml_bom_sample()).unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    let new_ver = Version::parse("1.0.1", VersionGrammar::SemVer).unwrap();
    manifest.write_version(&new_ver, &permit()).unwrap();
    manifest.persist(&permit()).unwrap();

    let updated_bytes = fs::read(&manifest_path).unwrap();
    let updated = String::from_utf8(updated_bytes).unwrap();

    assert!(
        updated.starts_with('\u{FEFF}'),
        "expected UTF-8 BOM to survive write, got:\n{updated:?}"
    );
    assert!(updated.contains("version = \"1.0.1\""));
}

/// F4 regression lock: a `Cargo.toml` with CRLF line endings (common on
/// Windows / `core.autocrlf=true`) must keep its CRLF line endings
/// across a `write_version` + `persist` round trip, not get silently
/// rewritten to bare LF. `toml_edit::DocumentMut::to_string()` always
/// renders bare-LF text, so `CargoToml` must detect and reapply the
/// original line ending itself -- exactly the fingerprinting `npm.rs`
/// and `python.rs` already had (and `cargo.rs` previously lacked).
#[test]
fn preserves_crlf_line_endings_on_write() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    fs::write(
        &manifest_path,
        callisto_fixtures::corpus::cargo_toml_crlf_no_bom_sample(),
    )
    .unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    assert_eq!(manifest.package_name().unwrap(), "crlf-crate");

    let new_ver = Version::parse("1.0.1", VersionGrammar::SemVer).unwrap();
    manifest.write_version(&new_ver, &permit()).unwrap();
    manifest.persist(&permit()).unwrap();

    let updated_bytes = fs::read(&manifest_path).unwrap();
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
    assert!(updated.contains("version = \"1.0.1\" # release version"));
}

#[test]
fn empty_cargo_toml_returns_parse_error_not_panic() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    fs::write(&manifest_path, "").unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    // An empty TOML document is technically valid (empty table); must not panic.
    let manifest = CargoToml::open(&decl, &ctx).unwrap();
    assert!(manifest.package_name().is_err());
}

#[test]
fn bom_only_cargo_toml_returns_parse_error_not_panic() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    fs::write(&manifest_path, "\u{FEFF}").unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    // BOM-only content strips down to an empty string, which parses as an
    // empty (valid) TOML document; must not panic.
    let manifest = CargoToml::open(&decl, &ctx).unwrap();
    assert!(manifest.package_name().is_err());
}

#[test]
fn garbage_cargo_toml_returns_parse_error_not_panic() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    fs::write(&manifest_path, "\u{FEFF}not valid toml {{{").unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let result = CargoToml::open(&decl, &ctx);
    assert!(matches!(result, Err(ManifestError::Parse { .. })));
}

#[test]
fn write_dependency_errors_on_unrecognized_value_shape_instead_of_silently_no_opping() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let content = "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.dependencies]\nserde = 1\n";
    fs::write(&manifest_path, content).unwrap();

    let mut resolver = WorkspaceCargoResolver::load(&manifest_path).unwrap();
    let new_spec = DepSpec::Range(
        VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
        "^1.1.0".to_string(),
    );
    let result = resolver.write_dependency("serde", new_spec, &permit());

    assert!(
        matches!(result, Err(ManifestError::UnrecognizedDependencyValue { .. })),
        "expected UnrecognizedDependencyValue, got: {result:?}"
    );

    let unchanged = fs::read_to_string(&manifest_path).unwrap();
    assert_eq!(
        unchanged, content,
        "an error return must not persist a stale/unchanged document to disk"
    );
}

/// Same as above but for the *outer* `else` branch (array-of-tables
/// dependency shape) -- see the identical case's comment on
/// `CargoToml::update_dependency_spec`'s own test for why this is a
/// distinct code path from a bare-value shape like `serde = 1`.
#[test]
fn write_dependency_errors_on_array_of_tables_dependency_shape() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let content = "[workspace]\nmembers = [\"crates/*\"]\n\n[[workspace.dependencies.serde]]\nversion = \"1.0.0\"\n";
    fs::write(&manifest_path, content).unwrap();

    let mut resolver = WorkspaceCargoResolver::load(&manifest_path).unwrap();
    let new_spec = DepSpec::Range(
        VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
        "^1.1.0".to_string(),
    );
    let result = resolver.write_dependency("serde", new_spec, &permit());

    assert!(
        matches!(result, Err(ManifestError::UnrecognizedDependencyValue { .. })),
        "expected UnrecognizedDependencyValue, got: {result:?}"
    );

    let unchanged = fs::read_to_string(&manifest_path).unwrap();
    assert_eq!(
        unchanged, content,
        "an error return must not persist a stale/unchanged document to disk"
    );
}

#[test]
fn write_dependency_preserves_trailing_comment_on_full_table_version() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let content = r#"[workspace]
members = ["crates/*"]

[workspace.dependencies.serde]
version = "1.0.0" # pinned intentionally, do not bump lightly
path = "../serde"
"#;
    fs::write(&manifest_path, content).unwrap();

    let mut resolver = WorkspaceCargoResolver::load(&manifest_path).unwrap();
    let new_spec = DepSpec::Range(
        VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
        "^1.1.0".to_string(),
    );
    resolver.write_dependency("serde", new_spec, &permit()).unwrap();
    resolver.persist(&permit()).unwrap();

    let updated = fs::read_to_string(&manifest_path).unwrap();
    assert!(updated.contains("version = \"^1.1.0\""));
    assert!(
        updated.contains("# pinned intentionally, do not bump lightly"),
        "expected trailing comment on version field to survive full-table write, got:\n{updated}"
    );
}

#[test]
fn write_dependency_preserves_decor_on_inline_table_version() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    // Custom spacing decor around the `version` key/value inside the inline table.
    // TOML forbids `#` comments inside inline tables, so the fidelity-loss proxy
    // here is the leading/trailing whitespace decor attached to the version value,
    // which write_dependency must preserve just like it does for the plain-string
    // and full-table branches.
    let content = "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.dependencies]\nserde = { version =   \"1.0.0\"  , path = \"../serde\" }\n";
    fs::write(&manifest_path, content).unwrap();

    let mut resolver = WorkspaceCargoResolver::load(&manifest_path).unwrap();
    let new_spec = DepSpec::Range(
        VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
        "^1.1.0".to_string(),
    );
    resolver.write_dependency("serde", new_spec, &permit()).unwrap();
    resolver.persist(&permit()).unwrap();

    let updated = fs::read_to_string(&manifest_path).unwrap();
    assert!(updated.contains("^1.1.0"));
    assert!(
        updated.contains("version =   \"^1.1.0\"  ,"),
        "expected custom decor around version field to survive inline-table write, got:\n{updated}"
    );
}

#[test]
fn preserves_tab_indentation_cargo_toml() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    fs::write(
        &manifest_path,
        callisto_fixtures::corpus::cargo_toml_tab_indented_sample(),
    )
    .unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    assert_eq!(manifest.package_name().unwrap(), "tabbed-crate");

    let new_ver = Version::parse("1.0.1", VersionGrammar::SemVer).unwrap();
    manifest.write_version(&new_ver, &permit()).unwrap();
    manifest.persist(&permit()).unwrap();

    let updated = fs::read_to_string(&manifest_path).unwrap();
    assert!(updated.contains("\tversion = \"1.0.1\""));
    assert!(updated.contains("\tname = \"tabbed-crate\""));
}

#[test]
fn test_cargo_is_publishable() {
    let dir = tempdir().unwrap();
    let pub_false_path = dir.path().join("Cargo.toml");
    fs::write(
        &pub_false_path,
        "[package]\nname = \"dummy\"\nversion = \"0.1.0\"\npublish = false\n",
    )
    .unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let manifest = CargoToml::open(&decl, &ctx).unwrap();
    assert!(!manifest.is_publishable());
}

/// Regression lock for F25: `CargoToml` no longer overrides
/// `publish_targets()` -- it falls through to the `Manifest` trait's
/// default impl, which dispatches on `ecosystem()` (`Ecosystem::Cargo`
/// -> `CratesIo`, or `None` when not publishable). This test pins that
/// dispatch still produces the same output the deleted override did.
#[test]
fn publish_targets_uses_trait_default_dispatch_for_cargo() {
    let dir = tempdir().unwrap();

    let publishable_path = dir.path().join("publishable").join("Cargo.toml");
    fs::create_dir_all(publishable_path.parent().unwrap()).unwrap();
    fs::write(&publishable_path, "[package]\nname = \"dummy\"\nversion = \"0.1.0\"\n").unwrap();
    let decl = ManifestDecl::new(
        "publishable/Cargo.toml",
        ManifestRole::Canonical,
        ManifestFormat::CargoToml,
    )
    .unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };
    let manifest = CargoToml::open(&decl, &ctx).unwrap();
    assert_eq!(
        manifest.publish_targets(),
        vec![callisto_model::PublishTarget::CratesIo]
    );

    let unpublishable_path = dir.path().join("unpublishable").join("Cargo.toml");
    fs::create_dir_all(unpublishable_path.parent().unwrap()).unwrap();
    fs::write(
        &unpublishable_path,
        "[package]\nname = \"dummy\"\nversion = \"0.1.0\"\npublish = false\n",
    )
    .unwrap();
    let decl = ManifestDecl::new(
        "unpublishable/Cargo.toml",
        ManifestRole::Canonical,
        ManifestFormat::CargoToml,
    )
    .unwrap();
    let manifest = CargoToml::open(&decl, &ctx).unwrap();
    assert_eq!(manifest.publish_targets(), vec![callisto_model::PublishTarget::None]);
}

#[test]
fn round_trip_cargo_spec() {
    let req = VersionReq::parse("^1.0.0", Ecosystem::Cargo).unwrap();
    let spec = DepSpec::Range(req, "^1.0.0".to_string());
    let target = Version::parse("1.2.0", VersionGrammar::SemVer).unwrap();

    let updated = round_trip(&spec, &target).unwrap();
    if let DepSpec::Range(_, s) = updated {
        assert_eq!(s, "^1.2.0");
    } else {
        panic!("expected Range DepSpec");
    }
}

#[test]
fn test_cargo_inline_table_dependency_update() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let content = r#"[package]
name = "my-crate"
version = "0.1.0"
edition = "2021"

[dependencies]
helper = { version = "1.0.0", path = "../helper" }
"#;
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    let new_spec = DepSpec::Range(
        VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
        "^1.1.0".to_string(),
    );
    manifest
        .update_dependency_spec(
            "helper",
            callisto_model::DepKind::Runtime,
            &Default::default(),
            new_spec,
            &permit(),
        )
        .unwrap();
    manifest.persist(&permit()).unwrap();

    let updated = fs::read_to_string(&manifest_path).unwrap();
    assert!(updated.contains("version = \"^1.1.0\""));
    assert!(updated.contains("path = \"../helper\""));
}

/// A TOML-syntactically-valid but semantically malformed dependency
/// value (an integer, here -- Cargo itself would reject this at build
/// time, but callisto's own CST editor doesn't validate shape before
/// attempting a rewrite) must surface as an error, not a silent no-op
/// success that leaves the manifest byte-for-byte unchanged.
#[test]
fn update_dependency_spec_errors_on_unrecognized_value_shape_instead_of_silently_no_opping() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let content = "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n\n[dependencies]\nhelper = 1\n";
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    let new_spec = DepSpec::Range(
        VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
        "^1.1.0".to_string(),
    );
    let result = manifest.update_dependency_spec(
        "helper",
        callisto_model::DepKind::Runtime,
        &Default::default(),
        new_spec,
        &permit(),
    );

    assert!(
        matches!(result, Err(ManifestError::UnrecognizedDependencyValue { .. })),
        "expected UnrecognizedDependencyValue, got: {result:?}"
    );
}

/// Same fallback, but for the *outer* `else` branch: a dependency
/// entry keyed via TOML array-of-tables syntax (`[[dependencies.x]]`)
/// is neither `as_value_mut()` (it's not a bare value/inline table)
/// nor `as_table_mut()` (a `Vec<Table>`, not a `Table`) -- an even
/// more obscure but still syntactically-legal shape than a bare
/// integer.
#[test]
fn update_dependency_spec_errors_on_array_of_tables_dependency_shape() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let content =
        "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n\n[[dependencies.helper]]\nversion = \"1.0.0\"\n";
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    let new_spec = DepSpec::Range(
        VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
        "^1.1.0".to_string(),
    );
    let result = manifest.update_dependency_spec(
        "helper",
        callisto_model::DepKind::Runtime,
        &Default::default(),
        new_spec,
        &permit(),
    );

    assert!(
        matches!(result, Err(ManifestError::UnrecognizedDependencyValue { .. })),
        "expected UnrecognizedDependencyValue, got: {result:?}"
    );
}

#[test]
fn update_dependency_spec_does_not_touch_disk_until_persist_called() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let content = "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n\n[dependencies]\nhelper = \"1.0.0\"\n";
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    let new_spec = DepSpec::Range(
        VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
        "^1.1.0".to_string(),
    );
    manifest
        .update_dependency_spec(
            "helper",
            callisto_model::DepKind::Runtime,
            &Default::default(),
            new_spec,
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
    assert!(updated.contains("helper = \"^1.1.0\""));
}

#[test]
fn test_cargo_decor_comment_preservation() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let content = r#"[package]
name = "my-crate"
version = "0.1.0" # preserve this inline comment
edition = "2021"
"#;
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    let new_ver = Version::parse("0.2.0", VersionGrammar::SemVer).unwrap();
    manifest.write_version(&new_ver, &permit()).unwrap();
    manifest.persist(&permit()).unwrap();

    let updated = fs::read_to_string(&manifest_path).unwrap();
    assert!(updated.contains("version = \"0.2.0\" # preserve this inline comment"));
}

#[test]
fn update_dependency_spec_preserves_decor_on_full_table_version() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let content = r#"[package]
name = "my-crate"
version = "0.1.0"

[dependencies.helper]
version = "1.0.0" # pinned intentionally, do not bump lightly
path = "../helper"
"#;
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    let new_spec = DepSpec::Range(
        VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
        "^1.1.0".to_string(),
    );
    manifest
        .update_dependency_spec(
            "helper",
            callisto_model::DepKind::Runtime,
            &Default::default(),
            new_spec,
            &permit(),
        )
        .unwrap();
    manifest.persist(&permit()).unwrap();

    let updated = fs::read_to_string(&manifest_path).unwrap();
    assert!(updated.contains("version = \"^1.1.0\""));
    assert!(
        updated.contains("# pinned intentionally, do not bump lightly"),
        "expected trailing comment on version field to survive full-table write, got:\n{updated}"
    );
}

#[test]
fn test_cargo_dependency_version_update_decor_preservation() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let content = r#"[package]
name = "my-crate"
version = "0.1.0"

[dependencies]
helper = { version = "1.0.0", path = "../helper" } # dep comment
"#;
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    let new_spec = DepSpec::Range(
        VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
        "^1.1.0".to_string(),
    );
    manifest
        .update_dependency_spec(
            "helper",
            callisto_model::DepKind::Runtime,
            &Default::default(),
            new_spec,
            &permit(),
        )
        .unwrap();
    manifest.persist(&permit()).unwrap();

    let updated = fs::read_to_string(&manifest_path).unwrap();
    assert!(updated.contains("# dep comment"));
}

/// Regression test: applying a version bump to a workspace-inheriting member must
/// write an explicit pinned version on the member's [package] section without
/// touching [workspace.package] in the root Cargo.toml.
#[test]
fn write_version_pins_explicitly_on_workspace_inheriting_member() {
    use std::sync::Arc;

    let dir = tempdir().unwrap();

    // Workspace root: version "1.0.0" in [workspace.package]
    let root_cargo_path = dir.path().join("Cargo.toml");
    fs::write(
        &root_cargo_path,
        r#"[workspace]
members = ["member"]

[workspace.package]
version = "1.0.0"
"#,
    )
    .unwrap();

    // Member with version.workspace = true
    let member_dir = dir.path().join("member");
    fs::create_dir_all(&member_dir).unwrap();
    let member_cargo_path = member_dir.join("Cargo.toml");
    fs::write(
        &member_cargo_path,
        r#"[package]
name = "member-crate"
version.workspace = true
edition = "2021"
"#,
    )
    .unwrap();

    // Build workspace inheritance context
    let ws_resolver = WorkspaceCargoResolver::load(&root_cargo_path).unwrap();
    let inheritance = Arc::new(ws_resolver.inheritance().unwrap());

    let decl = ManifestDecl::new("member/Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: Some(inheritance),
        npm_workspace_kind: None,
    };

    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    // Precondition: version is inherited as 1.0.0
    assert_eq!(manifest.current_version().unwrap().render(), "1.0.0");

    // Apply bump to 1.1.0 on the MEMBER ONLY
    let new_ver = Version::parse("1.1.0", VersionGrammar::SemVer).unwrap();
    manifest.write_version(&new_ver, &permit()).unwrap();

    // Regression lock for the workspace-inherited-version branch:
    // write_version alone must not touch disk. The member file must
    // still read exactly as it was originally written -- still
    // `version.workspace = true`, not yet pinned.
    let member_before_persist = fs::read_to_string(&member_cargo_path).unwrap();
    assert_eq!(
        member_before_persist, "[package]\nname = \"member-crate\"\nversion.workspace = true\nedition = \"2021\"\n",
        "write_version on the inherited-version branch must not touch disk before persist() is called"
    );

    manifest.persist(&permit()).unwrap();

    // Workspace root must be UNCHANGED
    let root_updated = fs::read_to_string(&root_cargo_path).unwrap();
    assert!(
        root_updated.contains("version = \"1.0.0\""),
        "workspace root version must remain 1.0.0, got:\n{root_updated}"
    );
    assert!(
        !root_updated.contains("1.1.0"),
        "workspace root must NOT contain 1.1.0, got:\n{root_updated}"
    );

    // Member must have an explicit pinned version, not workspace = true
    let member_updated = fs::read_to_string(&member_cargo_path).unwrap();
    assert!(
        member_updated.contains("version = \"1.1.0\""),
        "member must have pinned version 1.1.0, got:\n{member_updated}"
    );
    assert!(
        !member_updated.contains("workspace = true"),
        "member must not retain workspace inheritance after pinning, got:\n{member_updated}"
    );
}

/// `current_version()`'s error path: a member declares
/// `version.workspace = true` but the workspace root has no
/// `[workspace.package] version` field to inherit from.
#[test]
fn current_version_errors_when_inherited_but_workspace_has_no_version() {
    use std::sync::Arc;

    let dir = tempdir().unwrap();

    let root_cargo_path = dir.path().join("Cargo.toml");
    fs::write(&root_cargo_path, "[workspace]\nmembers = [\"member\"]\n").unwrap();

    let member_dir = dir.path().join("member");
    fs::create_dir_all(&member_dir).unwrap();
    let member_cargo_path = member_dir.join("Cargo.toml");
    fs::write(
        &member_cargo_path,
        "[package]\nname = \"member-crate\"\nversion.workspace = true\nedition = \"2021\"\n",
    )
    .unwrap();

    let ws_resolver = WorkspaceCargoResolver::load(&root_cargo_path).unwrap();
    let inheritance = Arc::new(ws_resolver.inheritance().unwrap());

    let decl = ManifestDecl::new("member/Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: Some(inheritance),
        npm_workspace_kind: None,
    };
    let manifest = CargoToml::open(&decl, &ctx).unwrap();

    let err = manifest.current_version().unwrap_err();
    assert!(
        matches!(err, ManifestError::WorkspaceInherited { ref key, .. } if key == "version"),
        "expected WorkspaceInherited{{key: \"version\"}}, got {err:?}"
    );
}

/// `iter_dependencies()`'s inherited-dependency resolution: a member
/// declaring `dep.workspace = true` resolves to the workspace root's
/// declared spec for that dependency, and an inherited dependency the
/// workspace root does NOT declare resolves to `DepSpec::Opaque`
/// (unresolved) rather than panicking or silently dropping the entry.
#[test]
fn iter_dependencies_resolves_inherited_deps_and_flags_unresolved_ones() {
    use std::sync::Arc;

    let dir = tempdir().unwrap();

    let root_cargo_path = dir.path().join("Cargo.toml");
    fs::write(
        &root_cargo_path,
        "[workspace]\nmembers = [\"member\"]\n\n[workspace.dependencies]\nserde = \"1.0\"\n",
    )
    .unwrap();

    let member_dir = dir.path().join("member");
    fs::create_dir_all(&member_dir).unwrap();
    let member_cargo_path = member_dir.join("Cargo.toml");
    fs::write(
            &member_cargo_path,
            "[package]\nname = \"member-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = { workspace = true }\nunresolved-dep = { workspace = true }\n",
        )
        .unwrap();

    let ws_resolver = WorkspaceCargoResolver::load(&root_cargo_path).unwrap();
    let inheritance = Arc::new(ws_resolver.inheritance().unwrap());

    let decl = ManifestDecl::new("member/Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: Some(inheritance),
        npm_workspace_kind: None,
    };
    let manifest = CargoToml::open(&decl, &ctx).unwrap();

    let entries: Vec<_> = manifest.iter_dependencies().collect();

    let serde_entry = entries.iter().find(|e| e.name == "serde").expect("serde entry present");
    assert!(serde_entry.inherited, "serde must be marked inherited");
    assert_eq!(
        serde_entry.spec,
        DepSpec::Range(VersionReq::parse("1.0", Ecosystem::Cargo).unwrap(), "1.0".to_string()),
        "serde's spec must resolve to the workspace root's declared range"
    );

    let unresolved_entry = entries
        .iter()
        .find(|e| e.name == "unresolved-dep")
        .expect("unresolved-dep entry present");
    assert!(
        unresolved_entry.inherited,
        "unresolved-dep must still be marked inherited"
    );
    assert_eq!(
            unresolved_entry.spec,
            DepSpec::Opaque("workspace = true (unresolved)".to_string()),
            "a dependency inherited by a member but not declared in [workspace.dependencies] must resolve to an Opaque unresolved marker, not panic or silently drop"
        );
}

/// `update_dependency_spec`'s successful delegation path for an
/// inherited dependency: the write actually lands in the workspace
/// root's `[workspace.dependencies]` table, not the member's own file.
#[test]
fn update_dependency_spec_delegates_inherited_dependency_write_to_workspace_root() {
    use std::sync::Arc;

    let dir = tempdir().unwrap();

    let root_cargo_path = dir.path().join("Cargo.toml");
    fs::write(
        &root_cargo_path,
        "[workspace]\nmembers = [\"member\"]\n\n[workspace.dependencies]\nserde = \"1.0\"\n",
    )
    .unwrap();

    let member_dir = dir.path().join("member");
    fs::create_dir_all(&member_dir).unwrap();
    let member_cargo_path = member_dir.join("Cargo.toml");
    fs::write(
            &member_cargo_path,
            "[package]\nname = \"member-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = { workspace = true }\n",
        )
        .unwrap();

    let ws_resolver = WorkspaceCargoResolver::load(&root_cargo_path).unwrap();
    let inheritance = Arc::new(ws_resolver.inheritance().unwrap());

    let decl = ManifestDecl::new("member/Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: Some(inheritance),
        npm_workspace_kind: None,
    };
    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();

    let new_spec = DepSpec::Range(VersionReq::parse("2.0", Ecosystem::Cargo).unwrap(), "2.0".to_string());
    manifest
        .update_dependency_spec("serde", DepKind::Runtime, &Default::default(), new_spec, &permit())
        .unwrap();

    let root_updated = fs::read_to_string(&root_cargo_path).unwrap();
    assert!(
        root_updated.contains("serde = \"2.0\""),
        "workspace root's [workspace.dependencies] must be updated, got:\n{root_updated}"
    );

    let member_unchanged = fs::read_to_string(&member_cargo_path).unwrap();
    assert!(
        member_unchanged.contains("workspace = true"),
        "member's own file must be untouched -- the write goes to the workspace root, got:\n{member_unchanged}"
    );
}

/// `update_dependency_spec` rejects `DepKind::Peer` outright: Cargo has
/// no peer-dependency concept, so this must fail with
/// `DependencyNotFound` rather than attempting to write to a
/// nonexistent section.
#[test]
fn update_dependency_spec_rejects_peer_dep_kind() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    fs::write(
        &manifest_path,
        "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = \"1.0\"\n",
    )
    .unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };
    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();

    let new_spec = DepSpec::Range(VersionReq::parse("2.0", Ecosystem::Cargo).unwrap(), "2.0".to_string());
    let err = manifest
        .update_dependency_spec("serde", DepKind::Peer, &Default::default(), new_spec, &permit())
        .unwrap_err();

    assert!(
        matches!(
            err,
            ManifestError::DependencyNotFound {
                kind: DepKind::Peer,
                ..
            }
        ),
        "expected DependencyNotFound for Peer kind, got {err:?}"
    );
}

/// `WorkspaceCargoResolver::workspace_version()`'s two branches: `Some`
/// when `[workspace.package] version` is declared, `None` when the
/// workspace declares no shared version at all.
#[test]
fn workspace_version_reads_present_and_absent() {
    let dir = tempdir().unwrap();

    let with_version = dir.path().join("with-version.toml");
    fs::write(
        &with_version,
        "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"3.2.1\"\n",
    )
    .unwrap();
    let resolver = WorkspaceCargoResolver::load(&with_version).unwrap();
    assert_eq!(
        resolver.workspace_version().unwrap().map(|v| v.render().to_string()),
        Some("3.2.1".to_string())
    );

    let without_version = dir.path().join("without-version.toml");
    fs::write(&without_version, "[workspace]\nmembers = []\n").unwrap();
    let resolver = WorkspaceCargoResolver::load(&without_version).unwrap();
    assert_eq!(resolver.workspace_version().unwrap(), None);
}

/// `WorkspaceCargoResolver::write_version()` mutates in memory only --
/// an explicit `persist()` call is required to write the new
/// `[workspace.package] version` to disk, mirroring `CargoToml`'s
/// `write_version`/`persist` split (see
/// `write_version_does_not_touch_disk_until_persist_called` above).
#[test]
fn workspace_cargo_resolver_write_version_persists_to_disk() {
    let dir = tempdir().unwrap();
    let root_cargo_path = dir.path().join("Cargo.toml");
    fs::write(
        &root_cargo_path,
        "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    let mut resolver = WorkspaceCargoResolver::load(&root_cargo_path).unwrap();
    let new_ver = Version::parse("2.0.0", VersionGrammar::SemVer).unwrap();
    resolver.write_version(&new_ver, &permit()).unwrap();

    let unchanged = fs::read_to_string(&root_cargo_path).unwrap();
    assert!(
        unchanged.contains("version = \"1.0.0\""),
        "write_version alone must not write to disk, got:\n{unchanged}"
    );

    resolver.persist(&permit()).unwrap();

    let updated = fs::read_to_string(&root_cargo_path).unwrap();
    assert!(
        updated.contains("version = \"2.0.0\""),
        "workspace root must be updated to 2.0.0 on disk after persist(), got:\n{updated}"
    );
}

/// `round_trip`'s `<=` upper-bound clause uses a strictly-greater-than
/// comparison (a target exactly AT the bound satisfies `<=`), distinct
/// from a bare `<` clause's "not less than" check (already covered by
/// `round_trip_compound_range_crossing_upper_bound_returns_none`). Also
/// covers `DepSpec::CargoBare`'s passthrough (rewrites to the target
/// version directly, no range logic at all).
#[test]
fn round_trip_le_upper_bound_allows_target_exactly_at_bound() {
    let req = VersionReq::parse(">=1.0.0, <=2.0.0", Ecosystem::Cargo).unwrap();
    let spec = DepSpec::Range(req, ">=1.0.0, <=2.0.0".to_string());

    // Target exactly AT the <= bound must still be accepted.
    let target = Version::parse("2.0.0", VersionGrammar::SemVer).unwrap();
    let result = round_trip(&spec, &target).expect("target exactly at <= bound must satisfy it");
    match result {
        DepSpec::Range(_, rendered) => {
            assert_eq!(rendered, ">=2.0.0, <=2.0.0");
        }
        other => panic!("expected DepSpec::Range, got {other:?}"),
    }

    // One patch version past the <= bound must be declined.
    let target = Version::parse("2.0.1", VersionGrammar::SemVer).unwrap();
    assert_eq!(
        round_trip(&spec, &target),
        None,
        "target past a <= bound must be declined, not silently accepted"
    );
}

/// `DepSpec::CargoBare` round-trips to the target version directly,
/// with no range-rewriting logic involved.
#[test]
fn round_trip_cargo_bare_passes_through_target_version() {
    let old = Version::parse("1.0.0", VersionGrammar::SemVer).unwrap();
    let spec = DepSpec::CargoBare(old);
    let target = Version::parse("1.5.0", VersionGrammar::SemVer).unwrap();

    assert_eq!(round_trip(&spec, &target), Some(DepSpec::CargoBare(target)));
}

/// `round_trip` declines (`None`) a single-clause range whose remainder
/// contains a wildcard (`1.*`) -- rewriting the numeric portion while
/// leaving a literal `*` behind would produce a malformed version string.
#[test]
fn round_trip_single_clause_wildcard_declines_rewrite() {
    let req = VersionReq::parse("1.*", Ecosystem::Cargo).unwrap();
    let spec = DepSpec::Range(req, "1.*".to_string());
    let target = Version::parse("2.0.0", VersionGrammar::SemVer).unwrap();

    assert_eq!(round_trip(&spec, &target), None);
}

/// `round_trip`'s catch-all: an `Opaque` spec (unresolved/unparseable)
/// is never round-tripped -- returns `None` rather than fabricating a
/// range for a spec it never understood in the first place.
#[test]
fn round_trip_opaque_spec_declines_rewrite() {
    let spec = DepSpec::Opaque("workspace = true (unresolved)".to_string());
    let target = Version::parse("1.0.0", VersionGrammar::SemVer).unwrap();

    assert_eq!(round_trip(&spec, &target), None);
}

/// Spec: a "virtual workspace" root `Cargo.toml` (one that defines only
/// `[workspace]` and has no `[package]` section) must make `package_name()`
/// return `Err(ManifestError::MissingField)`. Virtual workspaces are common
/// in Cargo monorepos where the root manifest coordinates workspace members
/// but is not itself a publishable crate.
#[test]
fn virtual_workspace_cargo_toml_has_no_package_name() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let content = r#"[workspace]
members = ["crates/*"]

[workspace.package]
version = "0.1.0"
edition = "2021"

[workspace.dependencies]
serde = { version = "1.0", features = ["derive"] }
"#;
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };

    // A virtual workspace root has no [package] table: open must succeed
    // (the file is valid TOML), but package_name must return an error.
    let manifest = CargoToml::open(&decl, &ctx).unwrap();
    let result = manifest.package_name();
    assert!(
        matches!(
            result,
            Err(ManifestError::MissingField {
                field: "package.name",
                ..
            })
        ),
        "virtual workspace must return MissingField for package.name, got: {result:?}"
    );
}

/// Spec: a crate manifest that uses `version.workspace = true` must be
/// openable when a `WorkspaceCargoResolver` context is supplied.  Without
/// that context the open must fail with a descriptive `Read` error (not a
/// panic), because `workspace = true` without the resolver cannot be
/// resolved and silently falling through would produce wrong results.
#[test]
fn cargo_toml_with_workspace_version_requires_resolver_context() {
    let dir = tempdir().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let content = r#"[package]
name = "my-inherited-crate"
version.workspace = true
edition = "2021"
"#;
    fs::write(&manifest_path, content).unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();

    // Without a workspace resolver, open must return Err — not panic.
    let ctx_no_ws = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };
    let result = CargoToml::open(&decl, &ctx_no_ws);
    assert!(
        result.is_err(),
        "opening version.workspace = true without resolver must return Err"
    );
    let err_msg = result.err().unwrap().to_string();
    assert!(
        err_msg.contains("workspace"),
        "error message must mention 'workspace', got: {err_msg}"
    );

    // With a resolver that provides the workspace version, open must succeed
    // and current_version must return the workspace-inherited version.
    let root_cargo_path = dir.path().join("root_Cargo.toml");
    let root_content = r#"[workspace]
members = ["."]

[workspace.package]
version = "4.5.6"
"#;
    fs::write(&root_cargo_path, root_content).unwrap();

    let inheritance = WorkspaceCargoResolver::load(&root_cargo_path)
        .unwrap()
        .inheritance()
        .unwrap();

    let ctx_with_ws = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: Some(std::sync::Arc::new(inheritance)),
        npm_workspace_kind: None,
    };
    let manifest = CargoToml::open(&decl, &ctx_with_ws).unwrap();
    assert_eq!(
        manifest.package_name().unwrap(),
        "my-inherited-crate",
        "package name must be read from [package].name even with workspace version"
    );
    assert_eq!(
        manifest.current_version().unwrap().render(),
        "4.5.6",
        "current_version must be inherited from the workspace resolver"
    );
}

/// A compound Cargo range `>=1.0.0, <2.0.0` whose lower bound is updated
/// past the preserved upper bound (e.g., target `2.1.0`) must return `None`
/// rather than silently producing the impossible `>=2.1.0, <2.0.0`.
#[test]
fn round_trip_compound_range_crossing_upper_bound_returns_none() {
    let req = VersionReq::parse(">=1.0.0, <2.0.0", Ecosystem::Cargo).unwrap();
    let spec = DepSpec::Range(req, ">=1.0.0, <2.0.0".to_string());
    let target = Version::parse("2.1.0", VersionGrammar::SemVer).unwrap();

    let result = round_trip(&spec, &target);
    assert!(
        result.is_none(),
        "compound range crossing upper bound must return None, got: {result:?}"
    );
}

/// A compound Cargo range within the same major version is safe to rewrite.
#[test]
fn round_trip_compound_range_within_upper_bound_is_rewritten() {
    let req = VersionReq::parse(">=1.0.0, <2.0.0", Ecosystem::Cargo).unwrap();
    let spec = DepSpec::Range(req, ">=1.0.0, <2.0.0".to_string());
    let target = Version::parse("1.5.0", VersionGrammar::SemVer).unwrap();

    let result = round_trip(&spec, &target);
    match result {
        Some(DepSpec::Range(_, rendered)) => {
            assert_eq!(
                rendered, ">=1.5.0, <2.0.0",
                "lower clause must be rewritten to the target while the upper bound clause is preserved unchanged"
            );
        }
        other => panic!("compound range within upper bound must be rewritten to Some(..), got: {other:?}"),
    }
}

use proptest::prelude::*;
proptest! {
    #[test]
    fn proptest_cargo_toml_parse_never_panics(s in "\\PC*") {
        let _res = s.parse::<toml_edit::DocumentMut>();
    }
}

#[test]
fn update_dependency_spec_self_referential_workspace_delegation_returns_invariant_violation() {
    let dir = tempdir().unwrap();
    let root_cargo_path = dir.path().join("Cargo.toml");
    let content = "[workspace]\nmembers = [\".\"]\n\n[package]\nname = \"self-referential-root\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = { workspace = true }\n\n[workspace.dependencies]\nserde = \"1.0.0\"\n";
    fs::write(&root_cargo_path, content).unwrap();

    let inheritance = WorkspaceCargoResolver::load(&root_cargo_path)
        .unwrap()
        .inheritance()
        .unwrap();

    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: Some(Arc::new(inheritance)),
        npm_workspace_kind: None,
    };

    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();

    let new_spec = DepSpec::Range(
        VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
        "^1.1.0".to_string(),
    );
    let result = manifest.update_dependency_spec(
        "serde",
        callisto_model::DepKind::Runtime,
        &Default::default(),
        new_spec,
        &permit(),
    );

    assert!(
        matches!(result, Err(ManifestError::InvariantViolation { .. })),
        "expected InvariantViolation, got: {result:?}"
    );

    let on_disk = fs::read_to_string(&root_cargo_path).unwrap();
    assert_eq!(on_disk, content, "manifest file must be unchanged on disk");

    manifest.persist(&permit()).unwrap();
    let after_persist = fs::read_to_string(&root_cargo_path).unwrap();
    assert_eq!(
        after_persist, content,
        "in-memory document must not have been mutated by the failed call"
    );
}

#[test]
fn one_key_declared_in_two_tables_is_two_entries() {
    let dir = tempdir().unwrap();
    let manifest = open_cargo(
        dir.path(),
        "[package]\nname = \"a\"\nversion = \"0.1.0\"\n\n[dependencies]\nb = \"^1.0.0\"\n\n[target.'cfg(unix)'.dependencies]\nb = \"=1.0.0\"\n",
    );
    let entries: Vec<_> = manifest.iter_dependencies().collect();
    assert_eq!(entries.len(), 2, "{entries:?}");
}

#[test]
fn update_dependency_spec_refuses_a_python_locator() {
    let dir = tempdir().unwrap();
    let content = "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n\n[dependencies]\nhelper = \"1.0.0\"\n";
    fs::write(dir.path().join("Cargo.toml"), content).unwrap();
    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };
    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    let spec = DepSpec::Range(
        VersionReq::parse("^1.1.0", Ecosystem::Cargo).unwrap(),
        "^1.1.0".to_string(),
    );
    let at = callisto_model::DepLocator::python(callisto_model::DepContainer::ProjectDependencies, Some(0));
    let result = manifest.update_dependency_spec("helper", callisto_model::DepKind::Runtime, &at, spec, &permit());
    assert!(
        matches!(result, Err(ManifestError::DependencyNotFound { .. })),
        "{result:?}"
    );
}
