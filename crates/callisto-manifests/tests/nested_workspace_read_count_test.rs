//! Opening every member of a Cargo workspace below the root reads that workspace once, not once per member.
//! Isolated in its own integration-test binary because the resolver-load counter is process-global.

use std::fs;

use callisto_manifests::{open, OpenContext};
use callisto_model::{ManifestDecl, ManifestFormat, ManifestRole};

#[test]
fn a_nested_workspace_is_loaded_once_however_many_members_inherit_from_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("native")).unwrap();
    fs::write(
        root.join("native/Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\n[workspace.package]\nversion = \"0.3.0\"\n[workspace.dependencies]\n\
         shared = \"1\"\n",
    )
    .unwrap();
    let decls: Vec<ManifestDecl> = (0..30)
        .map(|index| {
            let path = format!("native/crates/m{index}/Cargo.toml");
            fs::create_dir_all(root.join(&path).parent().unwrap()).unwrap();
            fs::write(
                root.join(&path),
                format!("[package]\nname = \"m{index}\"\nversion.workspace = true\n[dependencies]\nshared = {{ workspace = true }}\n"),
            )
            .unwrap();
            ManifestDecl::new(path, ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap()
        })
        .collect();

    let ctx = OpenContext::for_workspace_root(root).unwrap();
    callisto_manifests::reset_resolver_load_call_count();
    for decl in &decls {
        let manifest = open(decl, &ctx).unwrap();
        assert_eq!(manifest.current_version().unwrap().render(), "0.3.0");
    }
    assert_eq!(callisto_manifests::resolver_load_call_count(), 1);
}
