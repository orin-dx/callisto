/// Verifies that a Cargo workspace Cargo.toml at an absolute path can be
/// loaded by `WorkspaceCargoResolver` without error, and that the
/// `[workspace.package]` version field is accessible via the inheritance API.
#[test]
fn test_absolute_path_workspace_cargo_resolver() {
    let temp_dir = tempfile::tempdir().unwrap();
    let root_cargo = temp_dir.path().join("Cargo.toml");
    let content = r#"[workspace]
members = ["crates/sub"]
resolver = "2"

[workspace.package]
version = "0.2.0"
"#;
    std::fs::write(&root_cargo, content).unwrap();

    let resolver = callisto_manifests::WorkspaceCargoResolver::load(&root_cargo);
    assert!(resolver.is_ok());

    let inh = resolver.unwrap().inheritance().unwrap();
    assert_eq!(inh.version.unwrap().render(), "0.2.0");
}
