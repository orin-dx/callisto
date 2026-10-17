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

#[test]
fn renamed_dependencies_resolve_native_packages_and_write_declared_keys() {
    use callisto_manifests::{CargoToml, Manifest, OpenContext};
    use callisto_model::{ApplyPermit, DepKind, DepSpec, ManifestDecl, ManifestFormat, ManifestRole, Version};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Cargo.toml");
    let source = r#"[package]
name = "consumer"
version = "1.0.0"

[dependencies]
local_alias = { package = "actual-package", version = "^1.0", optional = true, features = ["keep"] } # keep

[dev-dependencies]
dev_alias = { package = "actual-package", version = "=1.0.0" }

[build-dependencies.build_alias]
package = "actual-package"
version = "~1.0" # build
"#;
    std::fs::write(&path, source).unwrap();
    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext::for_workspace_root(dir.path()).unwrap();
    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    let entries: Vec<_> = manifest.iter_dependencies().collect();

    assert_eq!(entries.len(), 3);
    for (name, kind) in [
        ("local_alias", DepKind::Optional),
        ("dev_alias", DepKind::Dev),
        ("build_alias", DepKind::Build),
    ] {
        let entry = entries.iter().find(|entry| entry.name == name).unwrap();
        assert_eq!(entry.package_name, "actual-package");
        assert_eq!(entry.kind, kind);
        assert!(!entry.inherited);
    }

    let permit = ApplyPermit::force_for_tests();
    for entry in &entries {
        manifest
            .update_dependency_spec(
                &entry.name,
                entry.kind,
                &Default::default(),
                DepSpec::CargoBare(Version::semver(2, 0, 0)),
                &permit,
            )
            .unwrap();
    }
    manifest.persist(&permit).unwrap();
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        source
            .replace("\"^1.0\"", "\"2.0.0\"")
            .replace("\"=1.0.0\"", "\"2.0.0\"")
            .replace("\"~1.0\"", "\"2.0.0\"")
    );
}

#[test]
fn inherited_renames_keep_member_flags_and_update_only_root_aliases() {
    use callisto_manifests::{CargoToml, Manifest, OpenContext, WorkspaceCargoResolver};
    use callisto_model::{ApplyPermit, DepKind, DepSpec, ManifestDecl, ManifestFormat, ManifestRole, Version};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Cargo.toml");
    let member_dir = dir.path().join("member");
    std::fs::create_dir(&member_dir).unwrap();
    let member_path = member_dir.join("Cargo.toml");
    let root_source = r#"[workspace]
members = ["member"]

[workspace.dependencies]
inline_alias = { package = "actual-package", version = "^1.0", features = ["keep"] } # keep

[workspace.dependencies.table_alias]
package = "actual-package"
version = "~1.0" # table
"#;
    let member_source = r#"[package]
name = "consumer"
version = "1.0.0"

[dependencies]
inline_alias = { workspace = true, optional = true, features = ["member"] } # member

[dev-dependencies]
inline_alias.workspace = true

[build-dependencies.table_alias]
workspace = true
"#;
    std::fs::write(&root, root_source).unwrap();
    std::fs::write(&member_path, member_source).unwrap();
    let inheritance = WorkspaceCargoResolver::load(&root).unwrap().inheritance().unwrap();
    for alias in ["inline_alias", "table_alias"] {
        let inherited = inheritance.inherited(alias).unwrap();
        assert_eq!(inherited.package_name, "actual-package");
        assert_eq!(inherited.declared_in, std::path::Path::new("Cargo.toml"));
    }

    let decl = ManifestDecl::new("member/Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: Some(std::sync::Arc::new(inheritance)),
        npm_workspace_kind: None,
    };
    let mut manifest = CargoToml::open(&decl, &ctx).unwrap();
    let entries: Vec<_> = manifest.iter_dependencies().collect();
    assert_eq!(entries.len(), 3);
    for (name, kind) in [
        ("inline_alias", DepKind::Optional),
        ("inline_alias", DepKind::Dev),
        ("table_alias", DepKind::Build),
    ] {
        let entry = entries
            .iter()
            .find(|entry| entry.name == name && entry.kind == kind)
            .unwrap();
        assert_eq!(entry.package_name, "actual-package");
        assert!(entry.inherited);
        assert_eq!(
            entry.spec.render(),
            if name == "inline_alias" { "^1.0" } else { "~1.0" }
        );
    }

    let permit = ApplyPermit::force_for_tests();
    for entry in &entries {
        manifest
            .update_dependency_spec(
                &entry.name,
                entry.kind,
                &Default::default(),
                DepSpec::CargoBare(Version::semver(2, 0, 0)),
                &permit,
            )
            .unwrap();
    }
    assert_eq!(std::fs::read_to_string(member_path).unwrap(), member_source);
    assert_eq!(
        std::fs::read_to_string(root).unwrap(),
        root_source
            .replace("\"^1.0\"", "\"2.0.0\"")
            .replace("\"~1.0\"", "\"2.0.0\"")
    );
}

#[test]
fn cargo_binary_inventory_includes_explicit_and_auto_targets() {
    use callisto_manifests::{CargoToml, Manifest, OpenContext};
    use callisto_model::{ManifestDecl, ManifestFormat, ManifestRole};

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname='app'\nversion='1.0.0'\nedition='2021'\n\n[[bin]]\nname='custom'\npath='tools/entry.rs'\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src/bin/tool")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(dir.path().join("src/bin/serve.rs"), "fn main() {}\n").unwrap();
    std::fs::write(dir.path().join("src/bin/tool/main.rs"), "fn main() {}\n").unwrap();
    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let manifest = CargoToml::open(&decl, &OpenContext::for_workspace_root(dir.path()).unwrap()).unwrap();
    assert_eq!(manifest.bin_names(), ["app", "custom", "serve", "tool"]);
}

#[test]
fn cargo_autobins_false_disables_only_automatic_target_discovery() {
    use callisto_manifests::{CargoToml, Manifest, OpenContext};
    use callisto_model::{ManifestDecl, ManifestFormat, ManifestRole};

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname='app'\nversion='1.0.0'\nedition='2021'\nautobins=false\n\n[[bin]]\nname='custom'\npath='tools/entry.rs'\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src/bin")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(dir.path().join("src/bin/automatic.rs"), "fn main() {}\n").unwrap();
    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let manifest = CargoToml::open(&decl, &OpenContext::for_workspace_root(dir.path()).unwrap()).unwrap();
    assert_eq!(manifest.bin_names(), ["custom"]);
}

#[test]
fn open_context_for_a_root_with_no_manifests_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = callisto_manifests::OpenContext::for_workspace_root(dir.path()).unwrap();
    assert!(ctx.cargo_workspace.is_none());
    assert!(ctx.npm_workspace_kind.is_none());
}

/// A root manifest that cannot be understood must not become "no inheritance": every inheriting member would then
/// fail to open and silently drop out of the release.
#[test]
fn open_context_fails_for_a_root_manifest_that_cannot_be_read_or_parsed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[workspace\nmembers = [\n").unwrap();
    assert!(callisto_manifests::OpenContext::for_workspace_root(dir.path()).is_err());

    std::fs::write(dir.path().join("Cargo.toml"), b"[workspace]\nname = \"caf\xe9\"\n").unwrap();
    assert!(callisto_manifests::OpenContext::for_workspace_root(dir.path()).is_err());

    std::fs::remove_file(dir.path().join("Cargo.toml")).unwrap();
    std::fs::write(dir.path().join("package.json"), "{\"workspaces\": [],}").unwrap();
    assert!(callisto_manifests::OpenContext::for_workspace_root(dir.path()).is_err());
}

#[test]
fn open_context_reads_a_root_manifest_that_starts_with_a_byte_order_mark() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "\u{FEFF}[workspace]\nmembers = []\n[workspace.package]\nversion = \"1.2.3\"\n",
    )
    .unwrap();
    let ctx = callisto_manifests::OpenContext::for_workspace_root(dir.path()).unwrap();
    assert!(ctx.cargo_workspace.is_some());
}
