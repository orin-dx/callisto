/// Tests exercise the write primitives directly rather than through a
/// command handler, so they mint a permit without a dry-run flag to
/// consult. Every non-test caller must go through
/// `ApplyPermit::granted_unless_dry_run`.
fn permit() -> callisto_model::ApplyPermit {
    callisto_model::ApplyPermit::force_for_tests()
}
use super::*;
use callisto_model::ManifestFormat;
use proptest::prelude::*;
use tempfile::tempdir;

/// The locator `iter_dependencies` assigned to the first declaration naming `name`.
fn locator_of(manifest: &PyprojectToml, name: &str) -> DepLocator {
    manifest
        .iter_dependencies()
        .find(|entry| normalize_pypi_package_name(&entry.name) == normalize_pypi_package_name(name))
        .map(|entry| entry.locator)
        .unwrap()
}

#[test]
fn bin_names_read_project_scripts_keys() {
    let dir = tempdir().unwrap();
    let open = |content: &str| {
        fs::write(dir.path().join("pyproject.toml"), content).unwrap();
        let decl = ManifestDecl::new("pyproject.toml", ManifestRole::Canonical, ManifestFormat::PyprojectToml).unwrap();
        let ctx = OpenContext {
            workspace_root: dir.path(),
            cargo_workspace: None,
            cargo_roots: Default::default(),
            npm_workspace_kind: None,
        };
        PyprojectToml::open(&decl, &ctx).unwrap()
    };
    let project = "[project]\nname = \"tool\"\nversion = \"1.0.0\"\n";
    assert!(open(project).bin_names().unwrap().is_empty());
    let scripts = format!("{project}\n[project.scripts]\nalpha = \"tool:a\"\nbeta = \"tool:b\"\n");
    assert_eq!(open(&scripts).bin_names().unwrap(), ["alpha", "beta"]);
}

#[test]
fn python_package_name_reads_pep621_name() {
    let doc: toml_edit::DocumentMut = "[project]\nname = \"my-lib\"\nversion = \"1.0.0\"\n".parse().unwrap();
    assert_eq!(python_package_name(&doc), Some("my-lib"));
}

#[test]
fn python_package_name_falls_back_to_poetry_name() {
    let doc: toml_edit::DocumentMut = "[tool.poetry]\nname = \"my-poetry-lib\"\nversion = \"1.0.0\"\n"
        .parse()
        .unwrap();
    assert_eq!(python_package_name(&doc), Some("my-poetry-lib"));
}

#[test]
fn python_package_name_falls_back_to_flit_module() {
    let doc: toml_edit::DocumentMut = "[tool.flit.metadata]\nmodule = \"my_flit_lib\"\n".parse().unwrap();
    assert_eq!(python_package_name(&doc), Some("my_flit_lib"));
}

#[test]
fn python_package_name_prefers_pep621_over_poetry_and_flit() {
    let doc: toml_edit::DocumentMut = "[project]\nname = \"pep621-name\"\n\n[tool.poetry]\nname = \"poetry-name\"\n\n[tool.flit.metadata]\nmodule = \"flit-name\"\n"
            .parse()
            .unwrap();
    assert_eq!(python_package_name(&doc), Some("pep621-name"));
}

#[test]
fn python_package_name_returns_none_when_all_absent() {
    let doc: toml_edit::DocumentMut = "[build-system]\nrequires = []\n".parse().unwrap();
    assert_eq!(python_package_name(&doc), None);
}

#[test]
fn parses_pep621_pyproject_and_updates_version_with_comment_preservation() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");

    let input_content = r#"[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

# Main project metadata
[project]
name = "my-python-lib" # The package name
version = "0.3.1" # Current release version
description = "A polyglot library"
dependencies = [
    "requests>=2.28.0",
]
"#;

    fs::write(&pyproject_path, input_content).unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };

    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    assert_eq!(manifest.package_name().unwrap(), "my-python-lib");

    let current_v = manifest.current_version().unwrap();
    assert_eq!(current_v.render(), "0.3.1");

    let new_v = Version::parse("0.3.2", VersionGrammar::Pep440).unwrap();
    manifest.write_version(&new_v, &permit()).unwrap();
    manifest.persist(&permit()).unwrap();

    let updated_content = fs::read_to_string(&pyproject_path).unwrap();
    assert!(updated_content.contains("version = \"0.3.2\""));
    assert!(updated_content.contains("# Main project metadata"));
    assert!(updated_content.contains("# Current release version"));
}

#[test]
fn write_version_does_not_touch_disk_until_persist_called() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");
    let content = "[project]\nname = \"my-python-lib\"\nversion = \"0.3.1\"\n";
    fs::write(&pyproject_path, content).unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    let new_v = Version::parse("0.3.2", VersionGrammar::Pep440).unwrap();
    manifest.write_version(&new_v, &permit()).unwrap();

    let unchanged = fs::read_to_string(&pyproject_path).unwrap();
    assert_eq!(unchanged, content, "write_version alone must not write to disk");

    manifest.persist(&permit()).unwrap();
    let updated = fs::read_to_string(&pyproject_path).unwrap();
    assert!(updated.contains("version = \"0.3.2\""));
}

#[test]
fn handles_utf8_bom_pyproject_toml() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");

    let input_content = "\u{FEFF}[project]\nname = \"bom-lib\"\nversion = \"1.0.0\"\n";
    fs::write(&pyproject_path, input_content).unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };

    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    assert_eq!(manifest.package_name().unwrap(), "bom-lib");
    assert_eq!(manifest.current_version().unwrap().render(), "1.0.0");
}

#[test]
fn preserves_bom_and_crlf_line_endings_on_write() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");

    fs::write(
        &pyproject_path,
        callisto_fixtures::corpus::pyproject_toml_bom_crlf_sample(),
    )
    .unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };

    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    assert_eq!(manifest.package_name().unwrap(), "bom-crlf-lib");

    let new_v = Version::parse("1.0.1", VersionGrammar::Pep440).unwrap();
    manifest.write_version(&new_v, &permit()).unwrap();
    manifest.persist(&permit()).unwrap();

    let updated_bytes = fs::read(&pyproject_path).unwrap();
    let updated = String::from_utf8(updated_bytes).unwrap();

    assert!(
        updated.starts_with('\u{FEFF}'),
        "expected UTF-8 BOM to survive write, got:\n{updated:?}"
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
fn preserves_crlf_line_endings_without_bom_on_write() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");

    fs::write(
        &pyproject_path,
        callisto_fixtures::corpus::pyproject_toml_crlf_no_bom_sample(),
    )
    .unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };

    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    assert_eq!(manifest.package_name().unwrap(), "crlf-lib");

    let new_v = Version::parse("1.0.1", VersionGrammar::Pep440).unwrap();
    manifest.write_version(&new_v, &permit()).unwrap();
    manifest.persist(&permit()).unwrap();

    let updated_bytes = fs::read(&pyproject_path).unwrap();
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
fn preserves_bom_without_crlf_on_write() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");

    fs::write(
        &pyproject_path,
        callisto_fixtures::corpus::pyproject_toml_bom_no_crlf_sample(),
    )
    .unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };

    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    assert_eq!(manifest.package_name().unwrap(), "bom-lf-lib");

    let new_v = Version::parse("1.0.1", VersionGrammar::Pep440).unwrap();
    manifest.write_version(&new_v, &permit()).unwrap();
    manifest.persist(&permit()).unwrap();

    let updated_bytes = fs::read(&pyproject_path).unwrap();
    let updated = String::from_utf8(updated_bytes).unwrap();

    assert!(
        updated.starts_with('\u{FEFF}'),
        "expected UTF-8 BOM to survive write, got:\n{updated:?}"
    );
    assert!(
        !updated.replace('\u{FEFF}', "").contains("\r\n"),
        "expected no CRLF to be introduced, got:\n{updated:?}"
    );
    assert!(updated.contains("version = \"1.0.1\" # release version"));
}

#[test]
fn empty_pyproject_toml_returns_parse_error_not_panic() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");
    fs::write(&pyproject_path, "").unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };

    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    // An empty TOML document is technically valid (empty table), so this
    // must not panic; the resulting manifest simply lacks required fields.
    let manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    assert!(manifest.package_name().is_err());
}

#[test]
fn bom_only_pyproject_toml_returns_parse_error_not_panic() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");
    fs::write(&pyproject_path, "\u{FEFF}").unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };

    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    // BOM-only content strips down to an empty string, which parses as an
    // empty (valid) TOML document; must not panic.
    let manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    assert!(manifest.package_name().is_err());
}

#[test]
fn garbage_pyproject_toml_returns_parse_error_not_panic() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");
    fs::write(&pyproject_path, "\u{FEFF}not valid toml {{{").unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };

    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let result = PyprojectToml::open(&decl, &ctx);
    assert!(matches!(result, Err(ManifestError::Parse { .. })));
}

#[test]
fn preserves_tab_indentation_pyproject_toml() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");

    fs::write(
        &pyproject_path,
        callisto_fixtures::corpus::pyproject_toml_tab_indented_sample(),
    )
    .unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };

    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    assert_eq!(manifest.package_name().unwrap(), "tabbed-lib");

    let new_v = Version::parse("1.0.1", VersionGrammar::Pep440).unwrap();
    manifest.write_version(&new_v, &permit()).unwrap();
    manifest.persist(&permit()).unwrap();

    let updated = fs::read_to_string(&pyproject_path).unwrap();
    assert!(updated.contains("\t\"requests>=2.28.0\""));
    assert!(updated.contains("version = \"1.0.1\""));
}

#[test]
fn parses_pep508_extras_and_markers() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");

    let input_content = r#"[project]
name = "complex-deps"
version = "0.1.0"
dependencies = [
    "requests[security]>=2.28.0; os_name == 'posix'",
    "urllib3<2.0.0",
]
"#;
    fs::write(&pyproject_path, input_content).unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };

    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    let deps: Vec<_> = manifest.iter_dependencies().collect();
    assert_eq!(deps.len(), 2);
    assert_eq!(deps[0].name, "requests");
    assert_eq!(deps[1].name, "urllib3");
}

// -- Poetry caret/tilde/wildcard normalization ---------------------------

#[test]
fn normalize_poetry_caret_full_triple() {
    assert_eq!(normalize_poetry_version_spec("^1.2.3"), ">=1.2.3,<2.0.0");
}

#[test]
fn normalize_poetry_caret_major_minor() {
    assert_eq!(normalize_poetry_version_spec("^1.2"), ">=1.2.0,<2.0.0");
}

#[test]
fn normalize_poetry_caret_major_only() {
    assert_eq!(normalize_poetry_version_spec("^1"), ">=1.0.0,<2.0.0");
}

#[test]
fn normalize_poetry_caret_leading_zero_major_pins_minor() {
    assert_eq!(normalize_poetry_version_spec("^0.2.3"), ">=0.2.3,<0.3.0");
}

#[test]
fn normalize_poetry_caret_leading_zero_major_and_minor_pins_patch() {
    assert_eq!(normalize_poetry_version_spec("^0.0.3"), ">=0.0.3,<0.0.4");
}

#[test]
fn normalize_poetry_caret_zero_major_minor_only() {
    assert_eq!(normalize_poetry_version_spec("^0.0"), ">=0.0.0,<0.1.0");
}

#[test]
fn normalize_poetry_caret_zero_major_only() {
    assert_eq!(normalize_poetry_version_spec("^0"), ">=0.0.0,<1.0.0");
}

#[test]
fn normalize_poetry_tilde_full_triple() {
    assert_eq!(normalize_poetry_version_spec("~1.2.3"), ">=1.2.3,<1.3.0");
}

#[test]
fn normalize_poetry_tilde_major_minor() {
    assert_eq!(normalize_poetry_version_spec("~1.2"), ">=1.2.0,<1.3.0");
}

#[test]
fn normalize_poetry_tilde_major_only() {
    assert_eq!(normalize_poetry_version_spec("~1"), ">=1.0.0,<2.0.0");
}

#[test]
fn normalize_poetry_bare_wildcard_matches_any() {
    assert_eq!(normalize_poetry_version_spec("*"), "");
}

#[test]
fn normalize_poetry_wildcard_major() {
    assert_eq!(normalize_poetry_version_spec("1.*"), ">=1.0.0,<2.0.0");
}

#[test]
fn normalize_poetry_wildcard_major_minor() {
    assert_eq!(normalize_poetry_version_spec("1.2.*"), ">=1.2.0,<1.3.0");
}

#[test]
fn normalize_poetry_leaves_pep440_operator_wildcard_untouched() {
    // "==1.2.*" is already valid PEP 440 (wildcard equality) -- must not
    // be mistaken for Poetry's bare wildcard syntax and rewritten.
    assert_eq!(normalize_poetry_version_spec("==1.2.*"), "==1.2.*");
}

#[test]
fn normalize_poetry_leaves_plain_pep440_range_untouched() {
    assert_eq!(normalize_poetry_version_spec(">=1.2,<1.5"), ">=1.2,<1.5");
}

#[test]
fn normalize_poetry_leaves_compatible_release_operator_untouched() {
    // "~=1.2.3" is PEP 440's own compatible-release operator, distinct
    // from Poetry's bare tilde "~1.2.3" -- must not be re-normalized.
    assert_eq!(normalize_poetry_version_spec("~=1.2.3"), "~=1.2.3");
}

#[test]
fn normalize_poetry_combines_caret_with_explicit_exclusion() {
    assert_eq!(normalize_poetry_version_spec("^1.2,!=1.2.5"), ">=1.2.0,<2.0.0,!=1.2.5");
}

#[test]
fn all_normalized_poetry_forms_parse_as_valid_pep440() {
    for raw in [
        "^1.2.3", "^0.2.3", "^0.0.3", "^0.0", "^0", "~1.2.3", "~1.2", "~1", "*", "1.*", "1.2.*",
    ] {
        let normalized = normalize_poetry_version_spec(raw);
        assert!(
            VersionReq::parse(&normalized, Ecosystem::Pypi).is_ok(),
            "expected `{raw}` (normalized to `{normalized}`) to parse as valid PEP 440"
        );
    }
}

// -- Poetry caret/tilde/wildcard dependency graph visibility -------------

fn poetry_manifest_with_dependency(dep_line: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");
    let content = format!(
            "[tool.poetry]\nname = \"my-app\"\nversion = \"0.1.0\"\n\n[tool.poetry.dependencies]\npython = \"^3.9\"\n{dep_line}\n"
        );
    fs::write(&pyproject_path, content).unwrap();
    (dir, pyproject_path)
}

fn open_poetry_manifest(dir: &tempfile::TempDir) -> PyprojectToml {
    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };
    PyprojectToml::open(&decl, &ctx).unwrap()
}

#[test]
fn poetry_caret_dependency_appears_in_dependency_graph_not_silently_dropped() {
    let (dir, _path) = poetry_manifest_with_dependency("upstream-lib = \"^1.2.3\"");
    let manifest = open_poetry_manifest(&dir);

    let deps: Vec<_> = manifest.iter_dependencies().collect();
    let entry = deps
        .iter()
        .find(|e| e.name == "upstream-lib")
        .expect("poetry caret dependency must appear in the dependency graph, not be silently dropped");

    let DepSpec::Range(req, raw) = &entry.spec else {
        panic!(
            "expected a Range spec carrying a usable VersionReq, got {:?}",
            entry.spec
        );
    };
    assert_eq!(
        raw, "^1.2.3",
        "original Poetry spec text should be preserved for display"
    );

    // The parsed constraint must actually gate the version-bump cascade
    // correctly, not merely exist: versions inside [1.2.3, 2.0.0) match,
    // versions outside do not.
    assert!(req.matches(&make_pep440_version("1.2.3")).unwrap());
    assert!(req.matches(&make_pep440_version("1.9.9")).unwrap());
    assert!(!req.matches(&make_pep440_version("1.2.2")).unwrap());
    assert!(!req.matches(&make_pep440_version("2.0.0")).unwrap());
}

#[test]
fn poetry_tilde_dependency_appears_in_dependency_graph() {
    let (dir, _path) = poetry_manifest_with_dependency("upstream-lib = \"~1.2.3\"");
    let manifest = open_poetry_manifest(&dir);

    let deps: Vec<_> = manifest.iter_dependencies().collect();
    let entry = deps
        .iter()
        .find(|e| e.name == "upstream-lib")
        .expect("poetry tilde dependency must appear in the dependency graph");

    let DepSpec::Range(req, _) = &entry.spec else {
        panic!("expected a Range spec, got {:?}", entry.spec);
    };
    assert!(req.matches(&make_pep440_version("1.2.9")).unwrap());
    assert!(!req.matches(&make_pep440_version("1.3.0")).unwrap());
}

#[test]
fn poetry_wildcard_dependency_appears_in_dependency_graph() {
    let (dir, _path) = poetry_manifest_with_dependency("upstream-lib = \"1.*\"");
    let manifest = open_poetry_manifest(&dir);

    let deps: Vec<_> = manifest.iter_dependencies().collect();
    let entry = deps
        .iter()
        .find(|e| e.name == "upstream-lib")
        .expect("poetry wildcard dependency must appear in the dependency graph");

    let DepSpec::Range(req, _) = &entry.spec else {
        panic!("expected a Range spec, got {:?}", entry.spec);
    };
    assert!(req.matches(&make_pep440_version("1.5.0")).unwrap());
    assert!(!req.matches(&make_pep440_version("2.0.0")).unwrap());
}

#[test]
fn poetry_bare_star_dependency_matches_any_version() {
    let (dir, _path) = poetry_manifest_with_dependency("upstream-lib = \"*\"");
    let manifest = open_poetry_manifest(&dir);

    let deps: Vec<_> = manifest.iter_dependencies().collect();
    let entry = deps
        .iter()
        .find(|e| e.name == "upstream-lib")
        .expect("poetry `*` dependency must appear in the dependency graph");

    let DepSpec::Range(req, _) = &entry.spec else {
        panic!("expected a Range spec, got {:?}", entry.spec);
    };
    assert!(req.matches(&make_pep440_version("0.0.1")).unwrap());
    assert!(req.matches(&make_pep440_version("99.0.0")).unwrap());
}

#[test]
fn poetry_inline_table_caret_dependency_appears_in_dependency_graph() {
    let (dir, _path) =
        poetry_manifest_with_dependency("upstream-lib = { version = \"^2.0\", extras = [\"speedups\"] }");
    let manifest = open_poetry_manifest(&dir);

    let deps: Vec<_> = manifest.iter_dependencies().collect();
    let entry = deps
        .iter()
        .find(|e| e.name == "upstream-lib")
        .expect("poetry inline-table caret dependency must appear in the dependency graph");
    assert!(matches!(entry.spec, DepSpec::Range(_, _)));
}

#[test]
fn poetry_dotted_table_dependency_appears_in_dependency_graph() {
    // Poetry also accepts dotted-table syntax as an alternative to inline
    // tables -- this parses to a different `toml_edit::Item` variant
    // (`Item::Table`, not `Item::Value(Value::InlineTable)`) and was
    // previously unhandled entirely, silently dropping the dependency.
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");
    let content = "[tool.poetry]\nname = \"my-app\"\nversion = \"0.1.0\"\n\n[tool.poetry.dependencies]\npython = \"^3.9\"\n\n[tool.poetry.dependencies.upstream-lib]\nversion = \"^1.5\"\n";
    fs::write(&pyproject_path, content).unwrap();
    let manifest = open_poetry_manifest(&dir);

    let deps: Vec<_> = manifest.iter_dependencies().collect();
    let entry = deps
        .iter()
        .find(|e| e.name == "upstream-lib")
        .expect("poetry dotted-table dependency must appear in the dependency graph, not be silently dropped");
    let DepSpec::Range(req, _) = &entry.spec else {
        panic!("expected a Range spec, got {:?}", entry.spec);
    };
    assert!(req.matches(&make_pep440_version("1.9.0")).unwrap());
    assert!(!req.matches(&make_pep440_version("2.0.0")).unwrap());
}

#[test]
fn poetry_git_dependency_without_version_appears_as_opaque_not_dropped() {
    let (dir, _path) = poetry_manifest_with_dependency(
        "upstream-lib = { git = \"https://example.com/upstream-lib.git\", tag = \"v1.0\" }",
    );
    let manifest = open_poetry_manifest(&dir);

    let deps: Vec<_> = manifest.iter_dependencies().collect();
    let entry = deps
        .iter()
        .find(|e| e.name == "upstream-lib")
        .expect("git dependency with no `version` key must still appear in the dependency graph");
    assert!(
        matches!(entry.spec, DepSpec::Opaque(_)),
        "expected an Opaque spec for a versionless git dependency, got {:?}",
        entry.spec
    );
}

#[test]
fn poetry_unrepresentable_spec_appears_as_opaque_not_dropped() {
    // A caret applied to a pre-release version core isn't decomposed by
    // this normalizer and isn't valid PEP 440 as-is either -- it must
    // still surface in the graph (as Opaque) rather than vanish.
    let (dir, _path) = poetry_manifest_with_dependency("upstream-lib = \"^1.2.3-beta\"");
    let manifest = open_poetry_manifest(&dir);

    let deps: Vec<_> = manifest.iter_dependencies().collect();
    let entry = deps
        .iter()
        .find(|e| e.name == "upstream-lib")
        .expect("an unrepresentable poetry spec must still appear in the dependency graph");
    assert!(
        matches!(entry.spec, DepSpec::Opaque(_)),
        "expected an Opaque fallback spec, got {:?}",
        entry.spec
    );
}

#[test]
fn pep621_unparseable_version_spec_appears_as_opaque_not_dropped() {
    // Sibling gap: the PEP 621 `[project].dependencies` block had the
    // same silent-drop-on-parse-failure pattern as the Poetry block.
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");
    let content = "[project]\nname = \"my-app\"\nversion = \"0.1.0\"\ndependencies = [\n    \"upstream-lib===not-a-real-version!!!\",\n]\n";
    fs::write(&pyproject_path, content).unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };
    let manifest = PyprojectToml::open(&decl, &ctx).unwrap();

    let deps: Vec<_> = manifest.iter_dependencies().collect();
    let entry = deps
        .iter()
        .find(|e| e.name == "upstream-lib")
        .expect("an unparseable PEP 621 spec must still appear in the dependency graph");
    assert!(
        matches!(entry.spec, DepSpec::Opaque(_)),
        "expected an Opaque fallback spec, got {:?}",
        entry.spec
    );
}

#[test]
fn updates_pep621_dependencies_array() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");

    let input_content = r#"[project]
name = "my-app"
version = "0.1.0"
dependencies = [
    "my-lib>=0.3.0",
]
"#;
    fs::write(&pyproject_path, input_content).unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };

    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    let req = VersionReq::parse(">=0.3.2", Ecosystem::Pypi).unwrap();
    let at = locator_of(&manifest, "my-lib");
    manifest
        .update_dependency_spec(
            "my-lib",
            DepKind::Runtime,
            &at,
            DepSpec::Range(req, ">=0.3.2".to_string()),
            &permit(),
        )
        .unwrap();

    manifest.persist(&permit()).unwrap();
    let updated_content = fs::read_to_string(&pyproject_path).unwrap();
    assert!(updated_content.contains("my-lib>=0.3.2"));
}

#[test]
fn update_dependency_spec_does_not_touch_disk_until_persist_called() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");
    let content = "[project]\nname = \"my-app\"\nversion = \"0.1.0\"\ndependencies = [\n    \"my-lib>=0.3.0\",\n]\n";
    fs::write(&pyproject_path, content).unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    let req = VersionReq::parse(">=0.3.2", Ecosystem::Pypi).unwrap();
    let at = locator_of(&manifest, "my-lib");
    manifest
        .update_dependency_spec(
            "my-lib",
            DepKind::Runtime,
            &at,
            DepSpec::Range(req, ">=0.3.2".to_string()),
            &permit(),
        )
        .unwrap();

    let unchanged = fs::read_to_string(&pyproject_path).unwrap();
    assert_eq!(
        unchanged, content,
        "update_dependency_spec alone must not write to disk"
    );

    manifest.persist(&permit()).unwrap();
    let updated = fs::read_to_string(&pyproject_path).unwrap();
    assert!(updated.contains("my-lib>=0.3.2"));
}

#[test]
fn update_dependency_spec_non_range_variant_is_ok_and_persist_is_a_no_op() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");
    let content = "[project]\nname = \"my-app\"\nversion = \"0.1.0\"\ndependencies = [\n    \"my-lib>=0.3.0\",\n]\n";
    fs::write(&pyproject_path, content).unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    let result = manifest.update_dependency_spec(
        "my-lib",
        DepKind::Runtime,
        &Default::default(),
        DepSpec::Opaque("path-dep".to_string()),
        &permit(),
    );
    assert!(
        result.is_ok(),
        "non-Range/non-Exact DepSpec must return Ok(()) without mutating"
    );

    manifest.persist(&permit()).unwrap();
    let on_disk = fs::read_to_string(&pyproject_path).unwrap();
    assert_eq!(
        on_disk, content,
        "persist of an untouched document must reproduce identical bytes"
    );
}

// -- round_trip tests ---------------------------------------------------

fn make_pypi_spec(raw: &str) -> DepSpec {
    let req = VersionReq::parse(raw, Ecosystem::Pypi).unwrap();
    DepSpec::Range(req, raw.to_string())
}

fn make_pep440_version(s: &str) -> Version {
    Version::parse(s, VersionGrammar::Pep440).unwrap()
}

fn raw_of(spec: Option<DepSpec>) -> Option<String> {
    match spec {
        Some(DepSpec::Range(_, raw)) => Some(raw),
        _ => None,
    }
}

/// The string `update_dependency_spec` writes for a rewritten range: the requirement's own rendering.
fn written_of(spec: Option<DepSpec>) -> Option<String> {
    match spec {
        Some(DepSpec::Range(req, _)) => Some(req.render().to_string()),
        _ => None,
    }
}

#[test]
fn round_trip_exact_pin_rewrites_to_new_version() {
    let spec = make_pypi_spec("==1.2.3");
    let target = make_pep440_version("2.0.0");
    assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some("==2.0.0"));
}

#[test]
fn round_trip_compatible_release_rewrites_to_new_version() {
    let spec = make_pypi_spec("~=1.4.2");
    let target = make_pep440_version("2.1.3");
    assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some("~=2.1.3"));
}

#[test]
fn a_compatible_release_clause_keeps_the_precision_it_was_written_with() {
    let target = make_pep440_version("2.1.3");
    // `~=1.2` is "any 1.x from 1.2"; rewriting it to `~=2.1.3` would silently shrink it to 2.1.x.
    assert_eq!(
        raw_of(round_trip(&make_pypi_spec("~=1.2"), &target)).as_deref(),
        Some("~=2.1")
    );
    assert_eq!(
        raw_of(round_trip(&make_pypi_spec("~=1.2.0"), &target)).as_deref(),
        Some("~=2.1.3")
    );
    assert_eq!(
        raw_of(round_trip(&make_pypi_spec("~= 1.2"), &target)).as_deref(),
        Some("~=2.1")
    );
}

/// Opens `source` as a `pyproject.toml`, rewrites `dependency` to `new`, and returns the persisted file.
fn poetry_rewrite(source: &str, dependency: &str, new: &str) -> String {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("pyproject.toml"), source).unwrap();
    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };
    let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    let at = locator_of(&manifest, dependency);
    manifest
        .update_dependency_spec(dependency, DepKind::Runtime, &at, make_pypi_spec(new), &permit())
        .unwrap();
    manifest.persist(&permit()).unwrap();
    fs::read_to_string(dir.path().join("pyproject.toml")).unwrap()
}

#[test]
fn rewriting_a_poetry_dependency_keeps_the_comments_around_it() {
    let source = "[tool.poetry]\nname = \"app\"\nversion = \"0.1.0\"\n\n[tool.poetry.dependencies]\npython = \"^3.10\"\n# the core library, pinned compatibly\nzz-core = \"~=1.2\"  # compatible release\nother = \"^1.0\"  # untouched\n";
    let rewritten = poetry_rewrite(source, "zz-core", "~=2.0");
    assert_eq!(rewritten, source.replace("~=1.2", "~=2.0"));
}

#[test]
fn rewriting_a_pep_621_dependency_keeps_the_comments_around_it() {
    let source = "[project]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\n    # the core library\n    \"zz-core~=1.2\",  # compatible release\n    \"other>=1.0\",  # untouched\n]\n";
    let rewritten = poetry_rewrite(source, "zz-core", "~=2.0");
    assert_eq!(rewritten, source.replace("~=1.2", "~=2.0"));
}

#[test]
fn rewriting_a_poetry_dependency_written_as_a_table_keeps_its_other_fields_and_comments() {
    let source = "[tool.poetry]\nname = \"app\"\nversion = \"0.1.0\"\n\n[tool.poetry.dependencies]\n# optional extras\nzz-core = { version = \"~=1.2\", extras = [\"fast\"] }  # pinned\n";
    let rewritten = poetry_rewrite(source, "zz-core", "~=2.0");
    assert_eq!(rewritten, source.replace("~=1.2", "~=2.0"));
}

#[test]
fn round_trip_lower_bound_only_rewrites_to_new_version() {
    let spec = make_pypi_spec(">=1.0.0");
    let target = make_pep440_version("2.5.1");
    assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some(">=2.5.1"));
}

#[test]
fn round_trip_range_rewrites_lower_and_computes_next_major_upper() {
    let spec = make_pypi_spec(">=1.0.0,<2");
    let target = make_pep440_version("2.5.1");
    assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some(">=2.5.1,<3"));
}

#[test]
fn round_trip_range_with_minor_upper_bound_computes_next_major() {
    let spec = make_pypi_spec(">=1.2.0,<2.0");
    let target = make_pep440_version("3.0.0");
    assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some(">=3.0.0,<4"));
}

#[test]
fn round_trip_prerelease_target_rewrites_to_full_version() {
    let spec = make_pypi_spec(">=1.0.0");
    let target = make_pep440_version("2.0.0a1");
    assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some(">=2.0.0a1"));
}

/// PEP 440 forbids a local version identifier on `>=`, so the lower bound carries the
/// target's public segments (release + dev) only; the local segment is dropped, not the
/// dev qualifier that keeps the bound below the eventual stable release.
#[test]
fn round_trip_dev_local_target_drops_local_segment_from_ordered_bound() {
    let spec = make_pypi_spec(">=1.0.0,<2.0");
    let target = make_pep440_version("0.0.0.dev0+canary.abc1234");
    assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some(">=0.0.0.dev0,<1"));
}

/// `==` is local-compatible, so an exact pin keeps the snapshot's local segment in full.
#[test]
fn round_trip_dev_local_target_exact_pin_preserves_the_full_identifier() {
    let spec = make_pypi_spec("==1.0.0");
    let target = make_pep440_version("0.0.0.dev0+canary.abc1234");
    assert_eq!(
        raw_of(round_trip(&spec, &target)).as_deref(),
        Some("==0.0.0.dev0+canary.abc1234")
    );
}

#[test]
fn round_trip_range_of_one_minor_stays_one_minor_wide() {
    let spec = make_pypi_spec(">=0.1.0,<0.2");
    let target = make_pep440_version("0.2.0");
    assert_eq!(raw_of(round_trip(&spec, &target)).as_deref(), Some(">=0.2.0,<0.3"));
}

#[test]
fn round_trip_poetry_caret_moves_to_the_new_version_as_an_equivalent_range() {
    let spec = DepSpec::Range(
        VersionReq::parse(">=0.1.0,<0.2.0", Ecosystem::Pypi).unwrap(),
        "^0.1.0".to_string(),
    );
    let target = make_pep440_version("0.2.0");
    assert_eq!(
        written_of(round_trip(&spec, &target)).as_deref(),
        Some(">=0.2.0, <0.3.0")
    );
}

#[test]
fn round_trip_poetry_tilde_moves_to_the_new_version_as_an_equivalent_range() {
    let spec = DepSpec::Range(
        VersionReq::parse(">=1.4.0,<1.5.0", Ecosystem::Pypi).unwrap(),
        "~1.4".to_string(),
    );
    let target = make_pep440_version("2.1.3");
    assert_eq!(
        written_of(round_trip(&spec, &target)).as_deref(),
        Some(">=2.1.3, <2.2.0")
    );
}

#[test]
fn round_trip_poetry_caret_is_left_alone_for_a_prerelease_target() {
    let spec = DepSpec::Range(
        VersionReq::parse(">=1.0.0,<2.0.0", Ecosystem::Pypi).unwrap(),
        "^1.0.0".to_string(),
    );
    let target = make_pep440_version("2.0.0a1");
    assert!(round_trip(&spec, &target).is_none());
}

#[test]
fn round_trip_upper_bound_only_returns_none() {
    let spec = make_pypi_spec("<2.0.0");
    let target = make_pep440_version("1.5.0");
    assert!(round_trip(&spec, &target).is_none());
}

#[test]
fn round_trip_exclusion_returns_none() {
    let spec = make_pypi_spec("!=1.5.0");
    let target = make_pep440_version("2.0.0");
    assert!(round_trip(&spec, &target).is_none());
}

#[test]
fn round_trip_non_range_spec_returns_none() {
    let target = make_pep440_version("2.0.0");
    let spec = DepSpec::Opaque("some-path-dep".to_string());
    assert!(round_trip(&spec, &target).is_none());
}

// -- bug regression tests -----------------------------------------------

#[test]
fn update_dependency_spec_returns_err_when_dep_not_found() {
    // Bug 1: if the dep name does not appear in any deps section the
    // function must return Err(DependencyNotFound), not Ok(()).
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");

    let input_content = r#"[project]
name = "my-app"
version = "0.1.0"
dependencies = [
    "requests>=2.28.0",
]
"#;
    fs::write(&pyproject_path, input_content).unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    let req = VersionReq::parse(">=1.0.0", Ecosystem::Pypi).unwrap();
    let result = manifest.update_dependency_spec(
        "nonexistent-dep",
        DepKind::Runtime,
        &Default::default(),
        DepSpec::Range(req, ">=1.0.0".to_string()),
        &permit(),
    );
    assert!(
        matches!(result, Err(ManifestError::DependencyNotFound { .. })),
        "expected DependencyNotFound, got: {result:?}"
    );
}

/// PEP 503: "-", "_", and "." are interchangeable in package names for
/// equality purposes. A dependent's pyproject.toml spelling a
/// requirement with a different-but-equivalent separator than the
/// depended-upon package's canonical name (as passed by the cascade
/// caller) must still be found and rewritten, not spuriously error as
/// DependencyNotFound.
#[test]
fn update_dependency_spec_matches_pep503_equivalent_separator_spelling() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");
    let content = "[project]\nname = \"my-app\"\nversion = \"0.1.0\"\ndependencies = [\n    \"my_lib>=0.3.0\",\n]\n";
    fs::write(&pyproject_path, content).unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    let req = VersionReq::parse(">=0.3.2", Ecosystem::Pypi).unwrap();
    let at = locator_of(&manifest, "my-lib");
    // Caller passes the canonical hyphenated name; the manifest spells
    // the requirement with an underscore.
    let result = manifest.update_dependency_spec(
        "my-lib",
        DepKind::Runtime,
        &at,
        DepSpec::Range(req, ">=0.3.2".to_string()),
        &permit(),
    );
    assert!(result.is_ok(), "expected Ok, got: {result:?}");

    manifest.persist(&permit()).unwrap();
    let updated = fs::read_to_string(&pyproject_path).unwrap();
    assert!(
        updated.contains(">=0.3.2"),
        "expected the underscore-spelled requirement to be rewritten, got:\n{updated}"
    );
}

#[test]
fn update_optional_dependencies_actually_writes_to_document() {
    // Bug 2: update_optional_dependencies must iterate
    // [project.optional-dependencies] and update matching entries instead
    // of being a no-op.
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");

    let input_content = r#"[project]
name = "my-app"
version = "0.1.0"
dependencies = []

[project.optional-dependencies]
docs = ["sphinx>=4.0.0"]
"#;
    fs::write(&pyproject_path, input_content).unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    let new_v = Version::parse("5.0.0", VersionGrammar::Pep440).unwrap();
    manifest
        .update_optional_dependencies(&[("sphinx".to_string(), new_v)], &permit())
        .unwrap();
    manifest.persist(&permit()).unwrap();

    let updated = fs::read_to_string(&pyproject_path).unwrap();
    assert!(
        updated.contains("sphinx>=5.0.0"),
        "expected sphinx to be updated to >=5.0.0, got:\n{updated}"
    );
}

/// PEP 503 equivalence for the optional-dependencies matcher too: an
/// update keyed by the canonical hyphenated name must still find a
/// requirement spelled with an underscore.
#[test]
fn update_optional_dependencies_matches_pep503_equivalent_separator_spelling() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");
    let content = "[project]\nname = \"my-app\"\nversion = \"0.1.0\"\ndependencies = []\n\n[project.optional-dependencies]\ndocs = [\"sphinx_rtd_theme>=4.0.0\"]\n";
    fs::write(&pyproject_path, content).unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    let new_v = Version::parse("5.0.0", VersionGrammar::Pep440).unwrap();
    // Caller passes the canonical hyphenated name; the manifest spells
    // the requirement with underscores.
    manifest
        .update_optional_dependencies(&[("sphinx-rtd-theme".to_string(), new_v)], &permit())
        .unwrap();
    manifest.persist(&permit()).unwrap();

    let updated = fs::read_to_string(&pyproject_path).unwrap();
    assert!(
        updated.contains("sphinx_rtd_theme>=5.0.0"),
        "expected sphinx_rtd_theme to be updated to >=5.0.0, got:\n{updated}"
    );
}

#[test]
fn update_optional_dependencies_does_not_touch_disk_until_persist_called() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");
    let content = "[project]\nname = \"my-app\"\nversion = \"0.1.0\"\ndependencies = []\n\n[project.optional-dependencies]\ndocs = [\"sphinx>=4.0.0\"]\n";
    fs::write(&pyproject_path, content).unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    let new_v = Version::parse("5.0.0", VersionGrammar::Pep440).unwrap();
    manifest
        .update_optional_dependencies(&[("sphinx".to_string(), new_v)], &permit())
        .unwrap();

    let unchanged = fs::read_to_string(&pyproject_path).unwrap();
    assert_eq!(
        unchanged, content,
        "update_optional_dependencies alone must not write to disk"
    );

    manifest.persist(&permit()).unwrap();
    let updated = fs::read_to_string(&pyproject_path).unwrap();
    assert!(updated.contains("sphinx>=5.0.0"));
}

/// Spec: `PyprojectToml` must support the `[tool.poetry]` schema as a
/// fallback when `[project]` is absent. Poetry is a popular Python build
/// backend; workspaces that have not migrated to PEP 621 use this schema.
/// Both `package_name()` and `current_version()` must delegate to
/// `[tool.poetry]` when `[project]` does not exist.
#[test]
fn poetry_pyproject_toml_is_detected_without_project_table() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");

    let content = r#"[build-system]
requires = ["poetry-core>=1.0.0"]
build-backend = "poetry.core.masonry.api"

[tool.poetry]
name = "my-poetry-pkg"
version = "3.1.4"
description = "A Poetry-managed package"

[tool.poetry.dependencies]
python = "^3.9"
requests = "^2.28.0"
"#;
    fs::write(&pyproject_path, content).unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    assert_eq!(
        manifest.package_name().unwrap(),
        "my-poetry-pkg",
        "package_name must read from [tool.poetry] when [project] is absent"
    );
    assert_eq!(
        manifest.current_version().unwrap().render(),
        "3.1.4",
        "current_version must read from [tool.poetry] when [project] is absent"
    );
}

/// Spec: a `pyproject.toml` that has neither a `[project]` table nor a
/// `[tool.poetry]` table must make `package_name()` return
/// `Err(ManifestError::MissingField)`. An absent `[project]` section is a
/// common scenario for build-system-only files (e.g. a pure
/// `pyproject.toml` used only for packaging metadata configuration).
#[test]
fn pyproject_toml_without_project_or_poetry_returns_missing_field_error() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");

    // Only a build-system section, no [project] and no [tool.poetry].
    let content = r#"[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.hatch.build]
exclude = ["tests/"]
"#;
    fs::write(&pyproject_path, content).unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    let result = manifest.package_name();
    assert!(
        matches!(result, Err(ManifestError::MissingField { .. })),
        "missing [project] and [tool.poetry] must return MissingField, got: {result:?}"
    );
    // current_version must also return an error, not panic.
    let ver_result = manifest.current_version();
    assert!(
        ver_result.is_err(),
        "current_version without [project] or [tool.poetry] must return Err"
    );
}

#[test]
fn round_trip_range_with_lte_upper_bound_returns_none() {
    // A "<=X.Y.Z" upper bound is not the recognized pattern; return None.
    let spec = make_pypi_spec(">=1.0.0,<=2.0.0");
    let target = make_pep440_version("1.5.0");
    assert!(round_trip(&spec, &target).is_none());
}

#[test]
fn round_trip_returns_none_for_three_clause_spec() {
    // A three-clause spec like ">=1.0.0,<2.0.0,!=1.5.0" must not be
    // silently rewritten — the third clause would be dropped, producing a
    // semantically different constraint.  round_trip must return None and
    // leave the decision to the user.
    let spec = make_pypi_spec(">=1.0.0,<2.0.0,!=1.5.0");
    let target = make_pep440_version("1.2.0");
    assert!(round_trip(&spec, &target).is_none());
}

// ---- F-07: is_publishable must be false for nameless pyproject.toml ----

fn open_pyproject(dir: &std::path::Path, content: &str) -> PyprojectToml {
    let path = dir.join("pyproject.toml");
    std::fs::write(&path, content).unwrap();
    let decl = callisto_model::ManifestDecl {
        path: std::path::PathBuf::from("pyproject.toml"),
        format: ManifestFormat::PyprojectToml,
        role: callisto_model::ManifestRole::Canonical,
    };
    let ctx = OpenContext {
        workspace_root: dir,
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };
    PyprojectToml::open(&decl, &ctx).unwrap()
}

/// A `pyproject.toml` that has no `[project].name` or `[tool.poetry].name`
/// is a workspace root or build-system-only file — it is not publishable.
/// Before this fix, `is_publishable` always returned `true` (trait default),
/// causing such files to produce a `Pypi` publish target and get included
/// in publish plans spuriously.
#[test]
fn pyproject_without_name_is_not_publishable() {
    let dir = tempdir().unwrap();
    let manifest = open_pyproject(
        dir.path(),
        "[build-system]\nrequires = [\"hatchling\"]\nbuild-backend = \"hatchling.build\"\n",
    );
    assert!(
        !manifest.is_publishable(),
        "pyproject.toml without [project].name must not be publishable"
    );
    assert_eq!(
        manifest.publish_targets(),
        vec![PublishTarget::None],
        "a non-publishable pyproject.toml must return PublishTarget::None"
    );
}

/// A `pyproject.toml` with `[project].name` IS publishable.
#[test]
fn pyproject_with_pep621_name_is_publishable() {
    let dir = tempdir().unwrap();
    let manifest = open_pyproject(dir.path(), "[project]\nname = \"my-pkg\"\nversion = \"1.0.0\"\n");
    assert!(
        manifest.is_publishable(),
        "pyproject.toml with [project].name must be publishable"
    );
    assert_eq!(manifest.publish_targets(), vec![PublishTarget::Pypi { index: None }],);
}

/// A `pyproject.toml` with `[tool.poetry].name` IS publishable (Poetry schema).
#[test]
fn pyproject_with_poetry_name_is_publishable() {
    let dir = tempdir().unwrap();
    let manifest = open_pyproject(
        dir.path(),
        "[tool.poetry]\nname = \"my-pkg\"\nversion = \"1.0.0\"\n\n[tool.poetry.dependencies]\npython = \"^3.10\"\n",
    );
    assert!(
        manifest.is_publishable(),
        "pyproject.toml with [tool.poetry].name must be publishable"
    );
}

/// Poetry 2.x projects can have a `[project]` table (for `requires-python`,
/// `name`, `dynamic = ["version"]`, etc.) WITHOUT a `version` field in
/// `[project]`.  The canonical version lives in `[tool.poetry].version`.
///
/// Before this fix, `write_version` dispatched on whether `[project]` exists
/// (not whether `[project].version` exists), so it inserted a new
/// `[project].version` key and left `[tool.poetry].version` at the old value.
/// `current_version()` then read the newly-inserted `[project].version` and
/// reported the write successful — but Poetry still published the old version.
#[test]
fn write_version_uses_poetry_section_when_project_table_has_no_version_field() {
    let dir = tempdir().unwrap();
    let content = "[project]\nname = \"my-project\"\nrequires-python = \">=3.9\"\ndynamic = [\"version\"]\n\n[tool.poetry]\nname = \"my-project\"\nversion = \"1.0.0\"\n\n[tool.poetry.dependencies]\npython = \"^3.9\"\n";
    let mut manifest = open_pyproject(dir.path(), content);

    let new_v = Version::parse("1.1.0", VersionGrammar::Pep440).unwrap();
    manifest.write_version(&new_v, &permit()).unwrap();
    manifest.persist(&permit()).unwrap();

    let on_disk = fs::read_to_string(dir.path().join("pyproject.toml")).unwrap();
    let doc: toml_edit::DocumentMut = on_disk.parse().unwrap();

    assert!(
        doc.get("project").and_then(|p| p.get("version")).is_none(),
        "[project].version must NOT be inserted when [project] has no version field;\
             got document:\n{on_disk}"
    );
    let poetry_ver = doc
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .expect("[tool.poetry].version must be present");
    assert_eq!(
        poetry_ver, "1.1.0",
        "[tool.poetry].version must be updated to 1.1.0; got document:\n{on_disk}"
    );
}

#[test]
fn persist_is_exposed_as_a_manifest_trait_method() {
    let dir = tempdir().unwrap();
    let pyproject_path = dir.path().join("pyproject.toml");
    let content = "[project]\nname = \"my-python-lib\"\nversion = \"0.3.1\"\n";
    fs::write(&pyproject_path, content).unwrap();

    let decl = ManifestDecl {
        path: PathBuf::from("pyproject.toml"),
        role: ManifestRole::Canonical,
        format: ManifestFormat::PyprojectToml,
    };
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        cargo_roots: Default::default(),
        npm_workspace_kind: None,
    };

    let mut manifest = PyprojectToml::open(&decl, &ctx).unwrap();
    <PyprojectToml as Manifest>::persist(&mut manifest, &permit()).unwrap();

    let after = fs::read_to_string(&pyproject_path).unwrap();
    assert_eq!(
        after, content,
        "persist() with no prior mutation must reproduce the file unchanged"
    );
}

#[test]
fn requirement_parse_extracts_name_extras_spec_and_marker() {
    let req = Requirement::parse("requests[security,socks]>=2.28.0; os_name == 'posix'").unwrap();
    assert_eq!(req.name, "requests");
    assert_eq!(req.extras, vec!["security".to_string(), "socks".to_string()]);
    assert_eq!(req.spec.as_deref(), Some(">=2.28.0"));
    assert_eq!(req.marker.as_deref(), Some("os_name == 'posix'"));
}

#[test]
fn requirement_parse_bare_name_has_no_extras_spec_or_marker() {
    let req = Requirement::parse("urllib3").unwrap();
    assert_eq!(req.name, "urllib3");
    assert!(req.extras.is_empty());
    assert_eq!(req.spec, None);
    assert_eq!(req.marker, None);
}

#[test]
fn requirement_parse_rejects_empty_name() {
    assert!(Requirement::parse("").is_none());
    assert!(Requirement::parse(">=1.0.0").is_none());
}

#[test]
fn requirement_render_round_trips_name_extras_spec_marker() {
    let req = Requirement {
        name: "requests".to_string(),
        extras: vec!["security".to_string(), "socks".to_string()],
        spec: Some(">=2.28.0".to_string()),
        marker: Some("os_name == 'posix'".to_string()),
    };
    assert_eq!(req.render(), "requests[security,socks]>=2.28.0;os_name == 'posix'");
    assert_eq!(Requirement::parse(&req.render()).unwrap(), req);
}

#[test]
fn requirement_parse_handles_dashed_and_dotted_names() {
    let req = Requirement::parse("sphinx-rtd-theme>=4.0.0").unwrap();
    assert_eq!(req.name, "sphinx-rtd-theme");
    assert_eq!(req.spec.as_deref(), Some(">=4.0.0"));
}

proptest! {
    /// Parsing an arbitrary string may or may not yield a `Requirement`
    /// (most random strings have no extractable package name), but
    /// whenever it does, rendering that result and parsing it again
    /// must reproduce an equal `Requirement` -- the shared parser
    /// reaches a stable, idempotent canonical form after one pass.
    #[test]
    fn requirement_parse_render_parse_is_idempotent(s in ".*") {
        if let Some(req) = Requirement::parse(&s) {
            let rendered = req.render();
            let reparsed = Requirement::parse(&rendered);
            prop_assert_eq!(reparsed, Some(req));
        }
    }
}

// -- one edge per requirement declaration ------------------------------

fn range(raw: &str) -> DepSpec {
    DepSpec::Range(VersionReq::parse(raw, Ecosystem::Pypi).unwrap(), raw.to_string())
}

fn rewrite_at(manifest: &mut PyprojectToml, name: &str, at: &DepLocator, raw: &str) -> Result<(), ManifestError> {
    let kind = if matches!(
        at,
        DepLocator::Python {
            container: DepContainer::Extra(_),
            ..
        }
    ) {
        DepKind::Optional
    } else {
        DepKind::Runtime
    };
    manifest.update_dependency_spec(name, kind, at, range(raw), &permit())
}

fn rendered(manifest: &PyprojectToml) -> String {
    manifest.document.to_string()
}

const TWO_EXTRAS: &str = "[project]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"b>=1.0; python_version<'3.9'\", \"b==1.0.0; python_version>='3.9'\"]\n\n[project.optional-dependencies]\na = [\"b>=1.0\"]\nc = [\"b==1.0.0\"]\n";

#[test]
fn every_requirement_declaration_is_its_own_edge_with_its_own_locator() {
    let dir = tempdir().unwrap();
    let manifest = open_pyproject(dir.path(), TWO_EXTRAS);
    let edges: Vec<_> = manifest
        .iter_dependencies()
        .map(|e| (e.kind, e.locator, e.spec.render()))
        .collect();
    assert_eq!(
        edges,
        [
            (
                DepKind::Runtime,
                DepLocator::python(DepContainer::ProjectDependencies, Some(0)),
                ">=1.0".to_string()
            ),
            (
                DepKind::Runtime,
                DepLocator::python(DepContainer::ProjectDependencies, Some(1)),
                "==1.0.0".to_string()
            ),
            (
                DepKind::Optional,
                DepLocator::python(DepContainer::Extra("a".to_string()), Some(0)),
                ">=1.0".to_string()
            ),
            (
                DepKind::Optional,
                DepLocator::python(DepContainer::Extra("c".to_string()), Some(0)),
                "==1.0.0".to_string()
            ),
        ]
    );
}

#[test]
fn a_position_counts_entries_the_graph_does_not_read() {
    let dir = tempdir().unwrap();
    let manifest = open_pyproject(
        dir.path(),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"\", \"a>=1\", \">=2\", \"b==1\"]\n",
    );
    let locators: Vec<_> = manifest.iter_dependencies().map(|e| (e.name, e.locator)).collect();
    let at = |position| DepLocator::python(DepContainer::ProjectDependencies, Some(position));
    assert_eq!(locators, [("a".to_string(), at(1)), ("b".to_string(), at(3))]);
}

#[test]
fn a_declaration_is_rewritten_by_its_locator_alone() {
    let dir = tempdir().unwrap();
    let mut manifest = open_pyproject(dir.path(), TWO_EXTRAS);
    let c = DepLocator::python(DepContainer::Extra("c".to_string()), Some(0));
    rewrite_at(&mut manifest, "b", &c, "==1.1.0").unwrap();
    assert_eq!(
        rendered(&manifest),
        TWO_EXTRAS.replace("c = [\"b==1.0.0\"]", "c = [\"b==1.1.0\"]")
    );
}

#[test]
fn marker_variants_are_rewritten_one_at_a_time_and_keep_their_markers() {
    let dir = tempdir().unwrap();
    let mut manifest = open_pyproject(dir.path(), TWO_EXTRAS);
    let first = DepLocator::python(DepContainer::ProjectDependencies, Some(0));
    let second = DepLocator::python(DepContainer::ProjectDependencies, Some(1));
    rewrite_at(&mut manifest, "b", &second, "==1.1.0").unwrap();
    assert!(rendered(&manifest).contains("\"b>=1.0; python_version<'3.9'\", \"b==1.1.0;python_version>='3.9'\""));
    rewrite_at(&mut manifest, "b", &first, ">=1.1.0").unwrap();
    assert!(rendered(&manifest).contains("\"b>=1.1.0;python_version<'3.9'\", \"b==1.1.0;python_version>='3.9'\""));
}

#[test]
fn the_same_requirement_in_each_container_is_rewritten_separately() {
    let dir = tempdir().unwrap();
    let mut manifest = open_pyproject(
        dir.path(),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"b==1.0.0\"]\n\n[project.optional-dependencies]\nx = [\"b==1.0.0\"]\n\n[tool.poetry.dependencies]\nb = \"==1.0.0\"\n",
    );
    let at: Vec<_> = manifest.iter_dependencies().map(|e| e.locator).collect();
    assert_eq!(at.len(), 3);
    rewrite_at(&mut manifest, "b", &at[1], "==2.0.0").unwrap();
    let after = rendered(&manifest);
    assert_eq!(after.matches("==2.0.0").count(), 1, "{after}");
    assert!(after.contains("x = [\"b==2.0.0\"]"), "{after}");
    rewrite_at(&mut manifest, "b", &at[2], "==3.0.0").unwrap();
    rewrite_at(&mut manifest, "b", &at[0], "==4.0.0").unwrap();
    let after = rendered(&manifest);
    assert!(after.contains("dependencies = [\"b==4.0.0\"]"), "{after}");
    assert!(after.contains("x = [\"b==2.0.0\"]"), "{after}");
    assert!(after.contains("b = \"==3.0.0\""), "{after}");
}

#[test]
fn extras_syntax_and_name_spellings_are_matched_and_kept() {
    let dir = tempdir().unwrap();
    let mut manifest = open_pyproject(
        dir.path(),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"Foo_Bar[x,y]>=1\", \"foo.bar==1\"]\n",
    );
    let at: Vec<_> = manifest.iter_dependencies().map(|e| e.locator).collect();
    rewrite_at(&mut manifest, "foo-bar", &at[1], "==2.0.0").unwrap();
    rewrite_at(&mut manifest, "foo-bar", &at[0], ">=2.0.0").unwrap();
    assert!(rendered(&manifest).contains("[\"Foo_Bar[x,y]>=2.0.0\", \"foo.bar==2.0.0\"]"));
}

#[test]
fn untouched_lines_and_comments_keep_their_formatting() {
    let dir = tempdir().unwrap();
    let content = "[project]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\n  # pinned for the api\n  \"b==1.0.0\",\n  \"other >= 1  \",   # keep me\n  \"b>=1.0\",\n]\n";
    let mut manifest = open_pyproject(dir.path(), content);
    let at = DepLocator::python(DepContainer::ProjectDependencies, Some(0));
    rewrite_at(&mut manifest, "b", &at, "==1.1.0").unwrap();
    assert_eq!(rendered(&manifest), content.replace("b==1.0.0", "b==1.1.0"));
}

#[test]
fn a_locator_naming_another_dependency_is_refused_and_writes_nothing() {
    let dir = tempdir().unwrap();
    let planned = open_pyproject(
        dir.path(),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"x==1\", \"b==1.0.0\"]\n",
    );
    let at = locator_of(&planned, "b");
    let changed = "[project]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"b==1.0.0\", \"x==1\"]\n";
    let mut manifest = open_pyproject(dir.path(), changed);
    let result = rewrite_at(&mut manifest, "b", &at, "==2.0.0");
    assert!(
        matches!(result, Err(ManifestError::DependencyNotFound { .. })),
        "{result:?}"
    );
    assert_eq!(rendered(&manifest), changed);
}

#[test]
fn a_locator_with_no_such_container_or_position_is_refused() {
    let dir = tempdir().unwrap();
    let content = "[project]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"b==1.0.0\"]\n\n[project.optional-dependencies]\nx = [\"b==1.0.0\"]\n";
    let mut manifest = open_pyproject(dir.path(), content);
    for at in [
        DepLocator::python(DepContainer::ProjectDependencies, Some(1)),
        DepLocator::python(DepContainer::ProjectDependencies, None),
        DepLocator::python(DepContainer::Extra("missing".to_string()), Some(0)),
        DepLocator::python(DepContainer::Extra("x".to_string()), Some(7)),
        DepLocator::python(DepContainer::PoetryDependencies, None),
        DepLocator::default(),
    ] {
        let result = rewrite_at(&mut manifest, "b", &at, "==2.0.0");
        assert!(
            matches!(result, Err(ManifestError::DependencyNotFound { .. })),
            "{at:?}: {result:?}"
        );
    }
    assert_eq!(rendered(&manifest), content);
}

#[test]
fn a_poetry_table_dependency_keeps_its_other_keys_when_rewritten() {
    let dir = tempdir().unwrap();
    let mut manifest = open_pyproject(
        dir.path(),
        "[tool.poetry]\nname = \"app\"\nversion = \"0.1.0\"\n\n[tool.poetry.dependencies]\npython = \"^3.9\"\n\n[tool.poetry.dependencies.b]\nversion = \"==1.0.0\"\nextras = [\"x\"]\n",
    );
    let at = locator_of(&manifest, "b");
    rewrite_at(&mut manifest, "b", &at, "==1.1.0").unwrap();
    let after = rendered(&manifest);
    assert!(
        after.contains("version = \"==1.1.0\"") && after.contains("extras = [\"x\"]"),
        "{after}"
    );
}

#[test]
fn update_dependency_spec_refuses_a_cargo_or_unspecified_locator() {
    let dir = tempdir().unwrap();
    let mut manifest = open_pyproject(dir.path(), TWO_EXTRAS);
    for at in [DepLocator::default(), DepLocator::cargo(DepKind::Runtime, None)] {
        let result = rewrite_at(&mut manifest, "b", &at, ">=2.0");
        assert!(
            matches!(result, Err(ManifestError::DependencyNotFound { .. })),
            "{at:?}: {result:?}"
        );
    }
    assert_eq!(rendered(&manifest), TWO_EXTRAS);
}
