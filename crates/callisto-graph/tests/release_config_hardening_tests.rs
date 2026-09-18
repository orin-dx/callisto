use std::fs;

const HEAD: &str = "[release]\nproduct-package = \"cargo/demo\"\nartifact-targets = [\"aarch64-apple-darwin\",\"x86_64-unknown-linux-gnu\",\"x86_64-unknown-linux-musl\",\"wasm32-wasip1\"]\n";

fn load(body: &str) -> Result<callisto_graph::ResolvedConfig, callisto_graph::error::ConfigError> {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("callisto.toml"), format!("{HEAD}{body}")).unwrap();
    load_config_at(d.path())
}
fn load_config_at(p: &std::path::Path) -> Result<callisto_graph::ResolvedConfig, callisto_graph::error::ConfigError> {
    callisto_graph::load_config(p)
}

#[test]
fn profiles_sharing_a_registry_endpoint_under_different_keys_are_rejected() {
    let e = load("[release.profiles.production]\nforge-repository=\"o/a\"\nregistry-routes={cratesIo=\"one\"}\n[release.profiles.rehearsal]\nforge-repository=\"o/b\"\nregistry-routes={cratesIo=\"two\"}\n[registries.one]\nkind=\"cargo\"\nurl=\"https://r.example.test/index\"\n[registries.two]\nkind=\"cargo\"\nurl=\"https://r.example.test/index\"\n");
    assert!(e.is_err(), "same endpoint under two keys is a shared destination");
}
#[test]
fn route_to_unknown_registry_is_rejected() {
    assert!(
        load("[release.profiles.production]\nforge-repository=\"o/a\"\nregistry-routes={cratesIo=\"missing\"}\n")
            .is_err()
    );
}
#[test]
fn custom_registry_route_without_url_is_rejected() {
    assert!(load("[release.profiles.production]\nforge-repository=\"o/a\"\nregistry-routes={cratesIo=\"one\"}\n[registries.one]\nkind=\"cargo\"\n").is_err());
}
#[test]
fn artifact_target_roster_must_be_exactly_the_four_supported_targets() {
    let d = tempfile::tempdir().unwrap();
    for targets in ["[\"aarch64-apple-darwin\"]", "[\"aarch64-apple-darwin\",\"aarch64-apple-darwin\",\"x86_64-unknown-linux-gnu\",\"wasm32-wasip1\"]", "[\"aarch64-apple-darwin\",\"x86_64-unknown-linux-gnu\",\"x86_64-unknown-linux-musl\",\"riscv64-unknown-linux-gnu\"]"] {
        fs::write(d.path().join("callisto.toml"), format!("[release]\nproduct-package = \"cargo/demo\"\nartifact-targets = {targets}\n")).unwrap();
        assert!(load_config_at(d.path()).is_err(), "{targets}");
    }
}
#[test]
fn invalid_profile_name_or_forge_repository_is_rejected() {
    assert!(load("[release.profiles.\"bad/name\"]\nforge-repository=\"o/a\"\n").is_err());
    assert!(load("[release.profiles.production]\nforge-repository=\"not a repo\"\n").is_err());
}
#[test]
#[ignore = "DEFECT-D07: a bare product-package is accepted and silently derives zero artifact slots"]
fn red_d07_product_package_must_be_ecosystem_qualified() {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("callisto.toml"), "[release]\nproduct-package = \"demo\"\nartifact-targets = [\"aarch64-apple-darwin\",\"x86_64-unknown-linux-gnu\",\"x86_64-unknown-linux-musl\",\"wasm32-wasip1\"]\n").unwrap();
    assert!(load_config_at(d.path()).is_err());
}
