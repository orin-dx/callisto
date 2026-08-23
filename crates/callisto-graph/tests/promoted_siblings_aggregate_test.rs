use std::collections::{BTreeMap, BTreeSet};

use callisto_graph::{GraphError, NoInference, TagIndex};
use callisto_model::{Ecosystem, PackageId, Version, VersionGrammar};

struct NoopRunner;
impl callisto_model::CommandRunner for NoopRunner {
    fn run(
        &self,
        _p: &str,
        _a: &[&str],
        _c: &std::path::Path,
    ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
        panic!("unused")
    }
}

#[test]
fn workspace_load_populates_promoted_siblings_and_aggregate_propagates_ambiguity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("crates/native-core")).unwrap();
    std::fs::write(
        root.join("crates/native-core/Cargo.toml"),
        "[package]\nname = \"native-core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("packages/native-core")).unwrap();
    std::fs::write(
        root.join("packages/native-core/package.json"),
        r#"{"name":"native-core","version":"0.1.0"}"#,
    )
    .unwrap();
    // No [[package]] rules at all -- byte-different from AC-19's fixture, so
    // Workspace::load itself succeeds; the ambiguous rule is injected onto a
    // cloned config afterward, proving aggregate() (not build()) is what
    // catches it.
    std::fs::write(root.join("callisto.toml"), "").unwrap();

    let locator = callisto_graph::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let workspace = callisto_graph::Workspace::load(root.to_path_buf(), &locator, &runner)
        .expect("Workspace::load must succeed for this AC-23 fixture");

    let siblings = workspace
        .config
        .promoted_siblings
        .get("native-core")
        .expect("promoted_siblings must contain the promoted name");
    assert_eq!(siblings.len(), 2, "exactly 2 distinct promoted ids for native-core");

    let cargo_entry = siblings
        .iter()
        .find(
            |(id, _)| matches!(id, PackageId::Prefixed { ecosystem: Ecosystem::Cargo, name } if name == "native-core"),
        )
        .expect("Cargo Prefixed entry must exist");
    let mut expected_cargo_set = BTreeSet::new();
    expected_cargo_set.insert(Ecosystem::Cargo);
    assert_eq!(
        cargo_entry.1, expected_cargo_set,
        "Cargo Prefixed entry's ecosystem set must be exactly {{Cargo}}, not a superset or a stale value"
    );

    let npm_entry = siblings
        .iter()
        .find(|(id, _)| matches!(id, PackageId::Prefixed { ecosystem: Ecosystem::Npm, name } if name == "native-core"))
        .expect("Npm Prefixed entry must exist");
    let mut expected_npm_set = BTreeSet::new();
    expected_npm_set.insert(Ecosystem::Npm);
    assert_eq!(
        npm_entry.1, expected_npm_set,
        "Npm Prefixed entry's ecosystem set must be exactly {{Npm}}, not a superset or a stale value"
    );

    let mut config2 = workspace.config.clone();
    let injected_id = PackageId::parse("native-core").unwrap();
    config2.packages.push((
        injected_id,
        callisto_graph::config::resolve::PackageConfig {
            release_trigger: None,
            publish_to: None,
            tag_template: None,
            changelog: None,
            pre_major_inference: None,
        },
    ));

    let git = workspace.git_access();
    let tags = TagIndex::empty();
    let mut base_versions = BTreeMap::new();
    base_versions.insert(
        PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "native-core".to_string(),
        },
        Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
    );
    base_versions.insert(
        PackageId::Prefixed {
            ecosystem: Ecosystem::Npm,
            name: "native-core".to_string(),
        },
        Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
    );
    let err = callisto_graph::aggregate(
        &workspace.graph,
        &config2,
        git,
        &tags,
        &base_versions,
        None,
        &NoInference,
    )
    .unwrap_err();
    match err {
        GraphError::AmbiguousName { name, candidates } => {
            assert_eq!(name, "native-core");
            assert_eq!(candidates.len(), 2);
        }
        other => panic!("expected AmbiguousName, got {other:?}"),
    }
}
