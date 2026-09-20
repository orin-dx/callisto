use callisto_graph::config::resolve::{product_asset_name, PRODUCT_ARTIFACT_TARGETS};

fn repo_file(relative: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

#[test]
fn asset_table_and_lookup_agree() {
    for (target, asset) in PRODUCT_ARTIFACT_TARGETS {
        assert_eq!(
            product_asset_name(target),
            Some(asset),
            "lookup disagrees with table for {target}"
        );
    }
    assert_eq!(product_asset_name("unsupported"), None);
}

#[test]
fn callisto_toml_artifact_targets_match_the_asset_table() {
    let parsed: toml::Table = repo_file("callisto.toml").parse().expect("callisto.toml parses");
    let configured: Vec<&str> = parsed["release"]["artifact-targets"]
        .as_array()
        .expect("release.artifact-targets is an array")
        .iter()
        .map(|value| value.as_str().expect("target is a string"))
        .collect();
    let table: Vec<&str> = PRODUCT_ARTIFACT_TARGETS.iter().map(|(target, _)| *target).collect();
    assert_eq!(
        configured, table,
        "callisto.toml [release] artifact-targets disagrees with PRODUCT_ARTIFACT_TARGETS in config/resolve.rs"
    );
}
