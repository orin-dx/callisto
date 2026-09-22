use std::fs;

const HEAD: &str = "[release]\nproduct-package = \"cargo/demo\"\n\n[[release.artifact]]\npackage = \"cargo/demo\"\ntarget = \"aarch64-apple-darwin\"\nasset-name = \"callisto-aarch64-apple-darwin.tar.gz\"\n\n[[release.artifact]]\npackage = \"cargo/demo\"\ntarget = \"x86_64-unknown-linux-gnu\"\nasset-name = \"callisto-x86_64-unknown-linux-gnu.tar.gz\"\n\n[[release.artifact]]\npackage = \"cargo/demo\"\ntarget = \"x86_64-unknown-linux-musl\"\nasset-name = \"callisto-x86_64-unknown-linux-musl.tar.gz\"\n\n[[release.artifact]]\npackage = \"cargo/demo\"\ntarget = \"wasm32-wasip1\"\nasset-name = \"callisto-moon.wasm\"\n";

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
fn artifact(package: &str, target: &str, asset: &str) -> String {
    format!("[[release.artifact]]\npackage = \"{package}\"\ntarget = \"{target}\"\nasset-name = \"{asset}\"\n")
}
fn release(artifacts: &str) -> String {
    format!("[release]\nproduct-package = \"cargo/demo\"\n{artifacts}")
}

/// No product identity is compiled in: any workspace may ship any targets under any names.
#[test]
fn a_non_callisto_product_with_its_own_targets_loads() {
    let d = tempfile::tempdir().unwrap();
    let body = release(
        &(artifact("cargo/demo", "x86_64-pc-windows-msvc", "demo-windows.zip")
            + &artifact("cargo/demo", "riscv64gc-unknown-linux-gnu", "demo-riscv.tar.gz")),
    );
    fs::write(d.path().join("callisto.toml"), body).unwrap();
    let config = load_config_at(d.path()).expect("generic artifact config loads");
    let assets: Vec<_> = config
        .product_release
        .unwrap()
        .artifacts
        .into_iter()
        .map(|a| a.asset_name)
        .collect();
    assert_eq!(assets, ["demo-windows.zip", "demo-riscv.tar.gz"]);
}

#[test]
fn invalid_artifact_declarations_are_rejected() {
    let d = tempfile::tempdir().unwrap();
    let cases = [
        ("empty", release("")),
        (
            "duplicate asset",
            release(&(artifact("cargo/demo", "a", "same.tar.gz") + &artifact("cargo/demo", "b", "same.tar.gz"))),
        ),
        (
            "path in asset",
            release(&artifact("cargo/demo", "a", "../escape.tar.gz")),
        ),
        ("path in target", release(&artifact("cargo/demo", "a/b", "ok.tar.gz"))),
        ("bare package", release(&artifact("demo", "a", "ok.tar.gz"))),
    ];
    for (label, body) in cases {
        fs::write(d.path().join("callisto.toml"), body).unwrap();
        assert!(load_config_at(d.path()).is_err(), "{label} must be rejected");
    }
}
#[test]
fn invalid_profile_name_or_forge_repository_is_rejected() {
    assert!(load("[release.profiles.\"bad/name\"]\nforge-repository=\"o/a\"\n").is_err());
    assert!(load("[release.profiles.production]\nforge-repository=\"not a repo\"\n").is_err());
}
#[test]
fn red_d07_product_package_must_be_ecosystem_qualified() {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("callisto.toml"), "[release]\nproduct-package = \"demo\"\n\n[[release.artifact]]\npackage = \"demo\"\ntarget = \"aarch64-apple-darwin\"\nasset-name = \"callisto-aarch64-apple-darwin.tar.gz\"\n\n[[release.artifact]]\npackage = \"demo\"\ntarget = \"x86_64-unknown-linux-gnu\"\nasset-name = \"callisto-x86_64-unknown-linux-gnu.tar.gz\"\n\n[[release.artifact]]\npackage = \"demo\"\ntarget = \"x86_64-unknown-linux-musl\"\nasset-name = \"callisto-x86_64-unknown-linux-musl.tar.gz\"\n\n[[release.artifact]]\npackage = \"demo\"\ntarget = \"wasm32-wasip1\"\nasset-name = \"callisto-moon.wasm\"\n").unwrap();
    assert!(load_config_at(d.path()).is_err());
}
