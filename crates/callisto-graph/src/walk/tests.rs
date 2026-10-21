use super::*;

fn write_pkg(root: &std::path::Path, rel: &str, eco: Ecosystem, name: &str) {
    std::fs::create_dir_all(root.join(rel)).unwrap();
    match eco {
        Ecosystem::Cargo => std::fs::write(
            root.join(rel).join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .unwrap(),
        Ecosystem::Npm => std::fs::write(
            root.join(rel).join("package.json"),
            format!(r#"{{"name":"{name}","version":"0.1.0"}}"#),
        )
        .unwrap(),
        Ecosystem::Pypi => std::fs::write(
            root.join(rel).join("pyproject.toml"),
            format!("[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
        )
        .unwrap(),
        _ => unreachable!(),
    }
}

#[test]
fn fixed_group_member_resolves_via_prefixed_unpromoted_cargo_package() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/foo", Ecosystem::Cargo, "foo");
    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"g\"\nmembers = [\"cargo/foo\"]\n",
    )
    .unwrap();
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner)
        .expect("Workspace::load must succeed for an unpromoted Cargo package referenced via cargo/foo");
    let native = ws
        .graph
        .identity()
        .native
        .get(&(Ecosystem::Cargo, "foo".to_string()))
        .expect("native entry must exist");
    let prefixed = ws
        .graph
        .identity()
        .prefixed
        .get(&(Ecosystem::Cargo, "foo".to_string()))
        .expect("prefixed entry must exist");
    assert_eq!(
        prefixed, native,
        "prefixed and native must resolve to the identical PackageId for an unpromoted single-ecosystem package"
    );
    let group = ws
        .config
        .groups
        .fixed
        .get(&callisto_model::GroupName("g".to_string()))
        .expect("group must exist");
    assert_eq!(group.members.len(), 1);
}

#[test]
fn fixed_group_member_naming_absent_ecosystem_is_missing_group_member() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/foo", Ecosystem::Cargo, "foo");
    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"g\"\nmembers = [\"npm/foo\"]\n",
    )
    .unwrap();
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let err = match crate::Workspace::load(root.to_path_buf(), &locator, &runner) {
        Err(e) => e,
        Ok(_) => panic!("expected MissingGroupMember error, got Ok"),
    };
    match err {
        GraphError::MissingGroupMember { member, .. } => {
            assert_eq!(member, "npm/foo");
        }
        other => panic!("expected MissingGroupMember, got {other:?}"),
    }
}

struct NoopRunner;
impl CommandRunner for NoopRunner {
    fn run(
        &self,
        _program: &str,
        _args: &[&str],
        _cwd: &Path,
    ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
        panic!("this test's workspace build never needs to shell out");
    }
}

/// End-to-end: a real disk-discovered napi platform manifest (co-located:
/// a `Cargo.toml` and a differently-named `package.json` sharing one
/// directory, the platform npm package's own identity distinct from the
/// owning crate's) must resolve through `[[fixed-group]] members`
/// naming the platform package by its own npm name, via
/// `IdentityIndex.platform` -- not the hand-constructed
/// `GroupMember::PlatformManifest` fixtures other tests use, which
/// bypass this wiring entirely and would not have caught the gap this
/// test pins.
#[test]
fn real_platform_manifest_resolves_via_fixed_group_and_feeds_napi_drift() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("package.json"),
        r#"{"name":"@myorg/my-crate-linux-x64-gnu","version":"0.1.0","os":["linux"],"cpu":["x64"]}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("callisto.toml"),
        "[[fixed-group]]\nname = \"my-group\"\nmembers = [\"cargo/my-crate\", \"@myorg/my-crate-linux-x64-gnu\"]\n",
    )
    .unwrap();

    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner)
        .expect("group resolution must succeed now that the real platform manifest resolves");

    let group = ws
        .config
        .groups
        .fixed
        .get(&callisto_model::GroupName("my-group".to_string()))
        .expect("fixed group must exist");

    let platform_member = group
        .members
        .iter()
        .find(|m| matches!(m, crate::config::groups::GroupMember::PlatformManifest { .. }))
        .expect("platform member must resolve, not be silently dropped or errored");

    let crate::config::groups::GroupMember::PlatformManifest { role, name, .. } = platform_member else {
        unreachable!()
    };
    assert_eq!(name, "@myorg/my-crate-linux-x64-gnu");
    assert_eq!(
        crate::napi::role_to_triple(role).as_deref(),
        Some("x86_64-unknown-linux-gnu"),
        "role must be the real, disk-derived role, not the old hardcoded \
             platform=\"unknown\" stub -- got: {role:?}"
    );

    // Feed straight into napi_drift, matching the task's own framing:
    // "napi_drift receives real group members".
    let declared = vec!["x86_64-unknown-linux-gnu".to_string()];
    let diagnostics = crate::napi::napi_drift(group, &declared, root);
    assert!(
        diagnostics.is_empty(),
        "declared napi.targets matches the real group member; expected no drift \
             diagnostics, got: {diagnostics:?}"
    );
}

/// A real `optionalDependencies` edge onto a co-located platform package (a
/// `Cargo.toml` and differently-named `package.json` sharing one
/// directory, per the test above) must resolve through
/// `IdentityIndex.native`, keyed by the platform manifest's own npm
/// name -- not the owning crate's `primary_id` name, which a
/// sibling's dependency entry never names. Before this fix,
/// `index.native` was keyed by `primary_id.name()` for every
/// ecosystem in a dual-manifest directory, so a dependency naming the
/// platform package by its real npm name silently failed to resolve,
/// dropping the edge with no diagnostic.
#[test]
fn optional_dependency_on_case_d_platform_package_resolves_via_native_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("package.json"),
        r#"{"name":"@myorg/my-crate-linux-x64-gnu","version":"0.1.0","os":["linux"],"cpu":["x64"]}"#,
    )
    .unwrap();

    std::fs::create_dir_all(root.join("consumer")).unwrap();
    std::fs::write(
            root.join("consumer/package.json"),
            r#"{"name":"@myorg/consumer","version":"0.1.0","optionalDependencies":{"@myorg/my-crate-linux-x64-gnu":"0.1.0"}}"#,
        )
        .unwrap();

    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load must succeed");

    let owning_crate = callisto_model::PackageId::Bare("my-crate".to_string());
    let consumer = callisto_model::PackageId::Bare("@myorg/consumer".to_string());

    let edge = ws.graph.edges().iter().find(|e| e.from == consumer).expect(
        "consumer's optionalDependencies edge onto the co-located platform package must \
                 resolve, not be silently dropped",
    );
    assert_eq!(
        edge.to, owning_crate,
        "the platform package belongs to the co-located owning crate; the edge must resolve \
             to the owning crate's identity, not fail to resolve at all"
    );
}

/// Real disk-discovered Linux napi platform packages always have
/// `abi: None` from raw `os`/`cpu` detection (npm's standard `os`/`cpu`
/// manifest fields have no libc/ABI concept), but `napi.rs::role_to_triple`'s
/// Linux match arms all require `Some("gnu")`/`Some("musl")`/
/// `Some("gnueabihf")` -- so before this fix, `role_to_triple` could
/// never resolve a real, disk-discovered Linux platform package to its
/// triple, and `napi_drift` would spuriously report it as a
/// declared-but-missing group member even though it's genuinely present
/// on disk with the correct name.
///
/// F23: role detection is now sourced from `Manifest::npm_role()` (the
/// already-cached handle), not a raw second `fs::read` -- these tests
/// drive the real `ManifestWalkResolver::build` path end to end (via
/// `Workspace::load`) to prove that wiring, rather than calling a
/// standalone `detect_npm_role` function directly (the pure ABI-inference
/// logic itself is now unit-tested in `callisto-manifests::npm`).
#[test]
fn build_infers_linux_gnu_abi_from_package_name_suffix_via_npm_role() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::write(
        root.join("pkg/package.json"),
        r#"{"name":"@scope/my-lib-linux-x64-gnu","version":"1.0.0","os":["linux"],"cpu":["x64"]}"#,
    )
    .unwrap();
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load must succeed");
    let (_, _, role) = ws
        .graph
        .identity()
        .platform
        .get("@scope/my-lib-linux-x64-gnu")
        .expect("platform entry must exist");

    assert_eq!(
        crate::napi::role_to_triple(role).as_deref(),
        Some("x86_64-unknown-linux-gnu"),
        "expected a resolvable triple, got role: {role:?}"
    );
}

#[test]
fn build_infers_linux_musl_abi_from_package_name_suffix_via_npm_role() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::write(
        root.join("pkg/package.json"),
        r#"{"name":"@scope/my-lib-linux-arm64-musl","version":"1.0.0","os":["linux"],"cpu":["arm64"]}"#,
    )
    .unwrap();
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load must succeed");
    let (_, _, role) = ws
        .graph
        .identity()
        .platform
        .get("@scope/my-lib-linux-arm64-musl")
        .expect("platform entry must exist");

    assert_eq!(
        crate::napi::role_to_triple(role).as_deref(),
        Some("aarch64-unknown-linux-musl"),
        "expected a resolvable triple, got role: {role:?}"
    );
}

#[test]
fn build_infers_linux_gnueabihf_abi_from_package_name_suffix_via_npm_role() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::write(
        root.join("pkg/package.json"),
        r#"{"name":"@scope/my-lib-linux-arm-gnueabihf","version":"1.0.0","os":["linux"],"cpu":["arm"]}"#,
    )
    .unwrap();
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load must succeed");
    let (_, _, role) = ws
        .graph
        .identity()
        .platform
        .get("@scope/my-lib-linux-arm-gnueabihf")
        .expect("platform entry must exist");

    assert_eq!(
        crate::napi::role_to_triple(role).as_deref(),
        Some("armv7-unknown-linux-gnueabihf"),
        "expected a resolvable triple, got role: {role:?}"
    );
}

/// Non-Linux platforms have no ABI concept in `role_to_triple`'s table
/// (`abi` is always `None` there) -- must not be affected by the
/// name-suffix inference at all.
#[test]
fn build_does_not_infer_abi_for_non_linux_platforms_via_npm_role() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::write(
        root.join("pkg/package.json"),
        r#"{"name":"@scope/my-lib-darwin-arm64-gnu","version":"1.0.0","os":["darwin"],"cpu":["arm64"]}"#,
    )
    .unwrap();
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load must succeed");
    let (_, _, role) = ws
        .graph
        .identity()
        .platform
        .get("@scope/my-lib-darwin-arm64-gnu")
        .expect("platform entry must exist");

    assert_eq!(
        *role,
        ManifestRole::Platform {
            platform: "darwin".to_string(),
            arch: "arm64".to_string(),
            abi: None,
        }
    );
}

/// A Linux platform package whose name has no recognized ABI suffix at
/// all stays `abi: None` -- this infers when it can, it doesn't invent an
/// ABI that isn't actually signaled anywhere.
#[test]
fn build_leaves_abi_none_when_linux_name_has_no_recognized_suffix_via_npm_role() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::write(
        root.join("pkg/package.json"),
        r#"{"name":"@scope/my-lib-linux-x64","version":"1.0.0","os":["linux"],"cpu":["x64"]}"#,
    )
    .unwrap();
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load must succeed");
    let (_, _, role) = ws
        .graph
        .identity()
        .platform
        .get("@scope/my-lib-linux-x64")
        .expect("platform entry must exist");

    assert_eq!(
        *role,
        ManifestRole::Platform {
            platform: "linux".to_string(),
            arch: "x64".to_string(),
            abi: None,
        }
    );
}

#[test]
fn claiming_ecosystems_is_name_scoped_not_full_path_ecosystem_set() {
    let list = vec![
        (Ecosystem::Cargo, PackageId::Bare("native-core".to_string())),
        (
            Ecosystem::Npm,
            PackageId::Bare("@myorg/native-core-linux-x64-gnu".to_string()),
        ),
    ];
    let primary_id = PackageId::Bare("native-core".to_string());
    let (claiming, native_keys) = compute_claiming_ecosystems_and_native_keys(&list, &primary_id);
    let mut expected = std::collections::BTreeSet::new();
    expected.insert(Ecosystem::Cargo);
    assert_eq!(
        claiming, expected,
        "npm Platform entry must NOT count toward the name-scoped claiming set"
    );
    assert_eq!(
        native_keys,
        vec![
            (Ecosystem::Cargo, "native-core".to_string()),
            (Ecosystem::Npm, "@myorg/native-core-linux-x64-gnu".to_string()),
        ],
        "path_native_keys must retain the COMPLETE unfiltered key set, unlike claiming_ecosystems"
    );
}

#[test]
fn primary_ecosystems_records_the_actual_primary_not_an_arbitrary_set_member() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/foo", Ecosystem::Cargo, "foo");
    std::fs::write(
        root.join("crates/foo/package.json"),
        r#"{"name":"@myorg/foo","version":"0.1.0"}"#,
    )
    .unwrap();
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("dual-manifest workspace must load");
    assert!(ws.graph.get(&PackageId::Bare("foo".to_string())).is_some());
}

#[test]
fn is_promoted_when_multiple_paths_claim_same_bare_name_with_disjoint_ecosystems() {
    let p1 = PathBuf::from("crates/native-core");
    let p2 = PathBuf::from("packages/native-core");
    let mut claiming = std::collections::BTreeMap::new();
    let mut eco1 = std::collections::BTreeSet::new();
    eco1.insert(Ecosystem::Cargo);
    let mut eco2 = std::collections::BTreeSet::new();
    eco2.insert(Ecosystem::Npm);
    claiming.insert(p1.clone(), eco1);
    claiming.insert(p2.clone(), eco2);
    let paths = vec![p1, p2];
    assert!(is_promoted_bare_name(&paths, &claiming));
}

#[test]
fn is_not_promoted_when_single_path_claims_bare_name() {
    let p1 = PathBuf::from("crates/native-core");
    let mut claiming = std::collections::BTreeMap::new();
    let mut eco1 = std::collections::BTreeSet::new();
    eco1.insert(Ecosystem::Cargo);
    claiming.insert(p1.clone(), eco1);
    let paths = vec![p1];
    assert!(!is_promoted_bare_name(&paths, &claiming));
}

#[test]
fn is_not_promoted_when_multiple_paths_have_overlapping_ecosystems() {
    let p1 = PathBuf::from("crates/native-core");
    let p2 = PathBuf::from("crates/other-core");
    let mut claiming = std::collections::BTreeMap::new();
    let mut eco1 = std::collections::BTreeSet::new();
    eco1.insert(Ecosystem::Cargo);
    let mut eco2 = std::collections::BTreeSet::new();
    eco2.insert(Ecosystem::Cargo);
    claiming.insert(p1.clone(), eco1);
    claiming.insert(p2.clone(), eco2);
    let paths = vec![p1, p2];
    assert!(
        !is_promoted_bare_name(&paths, &claiming),
        "overlapping ecosystem sets must NOT trigger promotion; duplicate check handles it"
    );
}

#[test]
fn promotion_predicate_is_disjointness_not_inequality() {
    let mut cargo_npm = BTreeSet::new();
    cargo_npm.insert(Ecosystem::Cargo);
    cargo_npm.insert(Ecosystem::Npm);
    let mut npm_only = BTreeSet::new();
    npm_only.insert(Ecosystem::Npm);
    let mut pypi_only = BTreeSet::new();
    pypi_only.insert(Ecosystem::Pypi);

    assert!(!claiming_sets_disjoint(&cargo_npm, &npm_only));
    assert!(claiming_sets_disjoint(&cargo_npm, &pypi_only));
    assert!(claiming_sets_disjoint(&npm_only, &pypi_only));
}

#[test]
fn same_ecosystem_collision_still_errors_unchanged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/a", Ecosystem::Cargo, "dup");
    write_pkg(root, "crates/b", Ecosystem::Cargo, "dup");
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let err = match crate::Workspace::load(root.to_path_buf(), &locator, &runner) {
        Err(e) => e,
        Ok(_) => panic!("expected DuplicatePackage error, got Ok"),
    };
    match err {
        GraphError::DuplicatePackage { id, paths } => {
            assert_eq!(id, PackageId::Bare("dup".to_string()));
            assert_eq!(paths.len(), 2);
        }
        other => panic!("expected DuplicatePackage, got {other:?}"),
    }
}

#[test]
fn case_d_colliding_with_third_disjoint_ecosystem_still_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/case-d", Ecosystem::Cargo, "hybrid");
    write_pkg(root, "crates/case-d", Ecosystem::Npm, "hybrid");
    write_pkg(root, "packages/npm-hybrid", Ecosystem::Npm, "hybrid");
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let err = match crate::Workspace::load(root.to_path_buf(), &locator, &runner) {
        Err(e) => e,
        Ok(_) => panic!("expected DuplicatePackage error, got Ok"),
    };
    match err {
        GraphError::DuplicatePackage { id, paths } => {
            assert_eq!(id, PackageId::Bare("hybrid".to_string()));
            assert_eq!(paths.len(), 2);
        }
        other => panic!("expected DuplicatePackage, got {other:?}"),
    }
}

/// A dual-manifest directory's non-primary native name (its npm name,
/// with Cargo as primary) colliding with a separate directory's only
/// name must still error, not silently overwrite `index.native` and
/// route dependency edges to the wrong package.
#[test]
fn duplicate_on_non_primary_native_name_still_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "a", Ecosystem::Cargo, "core");
    std::fs::write(root.join("a/package.json"), r#"{"name":"@x/core","version":"1.0.0"}"#).unwrap();
    write_pkg(root, "b", Ecosystem::Npm, "@x/core");
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let err = match crate::Workspace::load(root.to_path_buf(), &locator, &runner) {
        Err(e) => e,
        Ok(_) => panic!("expected DuplicatePackage error, got Ok"),
    };
    match err {
        GraphError::DuplicatePackage { id, paths } => {
            assert_eq!(id, PackageId::Bare("@x/core".to_string()));
            let paths: std::collections::BTreeSet<_> = paths.into_iter().collect();
            assert_eq!(
                paths,
                [PathBuf::from("a"), PathBuf::from("b")].into_iter().collect(),
                "must name both the dual-manifest directory and the colliding npm-only directory"
            );
        }
        other => panic!("expected DuplicatePackage, got {other:?}"),
    }
}

#[test]
fn disjoint_cross_ecosystem_collision_promotes_instead_of_duplicate_package() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/native-core", Ecosystem::Cargo, "native-core");
    write_pkg(root, "packages/native-core", Ecosystem::Npm, "native-core");

    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner)
        .expect("disjoint cross-ecosystem collision must promote, not DuplicatePackage");

    let cargo_id = PackageId::Prefixed {
        ecosystem: Ecosystem::Cargo,
        name: "native-core".to_string(),
    };
    let npm_id = PackageId::Prefixed {
        ecosystem: Ecosystem::Npm,
        name: "native-core".to_string(),
    };
    assert_eq!(ws.graph.packages().count(), 2);
    let cargo_pkg = ws.graph.get(&cargo_id).expect("Cargo-prefixed entry must exist");
    let npm_pkg = ws.graph.get(&npm_id).expect("Npm-prefixed entry must exist");
    assert_eq!(cargo_pkg.manifests.len(), 1);
    assert_eq!(
        cargo_pkg.manifests[0].path,
        PathBuf::from("crates/native-core/Cargo.toml")
    );
    assert_eq!(npm_pkg.manifests.len(), 1);
    assert_eq!(
        npm_pkg.manifests[0].path,
        PathBuf::from("packages/native-core/package.json")
    );
}

#[test]
fn promotion_changes_the_display_id_but_not_the_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/native-core", Ecosystem::Cargo, "native-core");
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let before = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("load");
    let unique = before
        .graph
        .get(&PackageId::Bare("native-core".to_string()))
        .expect("bare id")
        .clone();

    write_pkg(root, "packages/native-core", Ecosystem::Npm, "native-core");
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let after = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("load");
    let cargo_id = after.identity.resolve("cargo/native-core").expect("qualified selector");
    let promoted = after.graph.get(&cargo_id).expect("promoted package");

    assert_eq!(unique.id.display_name(), "native-core");
    assert_eq!(promoted.id.display_name(), "cargo/native-core");
    assert_eq!(promoted.key(), unique.key());
    assert_eq!(
        unique.key(),
        Some(callisto_model::PackageKey::new("crates/native-core"))
    );
    assert_eq!(
        before
            .identity
            .resolve("cargo/native-core")
            .expect("qualified before promotion"),
        unique.id
    );
}

#[test]
fn promoted_name_removed_from_index_bare() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/native-core", Ecosystem::Cargo, "native-core");
    write_pkg(root, "packages/native-core", Ecosystem::Npm, "native-core");
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("promotion must succeed");
    assert!(
        !ws.graph.identity().bare.contains_key("native-core"),
        "index.bare must not retain the stale pre-promotion key once both occurrences promote"
    );
    let unprefixed_lookup = ws.graph.identity().resolve_human("native-core", &[]);
    assert!(matches!(unprefixed_lookup, Err(GraphError::AmbiguousName { .. })));
}

#[test]
fn promoted_native_values_point_to_prefixed_ids() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/native-core", Ecosystem::Cargo, "native-core");
    write_pkg(root, "packages/native-core", Ecosystem::Npm, "native-core");
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("promotion must succeed");

    let cargo_native = ws
        .graph
        .identity()
        .native
        .get(&(Ecosystem::Cargo, "native-core".to_string()))
        .expect("Cargo native entry must exist");
    let npm_native = ws
        .graph
        .identity()
        .native
        .get(&(Ecosystem::Npm, "native-core".to_string()))
        .expect("Npm native entry must exist");

    assert_eq!(
        cargo_native,
        &PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "native-core".to_string(),
        },
        "index.native value must point to the promoted Cargo-prefixed ID"
    );
    assert_eq!(
        npm_native,
        &PackageId::Prefixed {
            ecosystem: Ecosystem::Npm,
            name: "native-core".to_string(),
        },
        "index.native value must point to the promoted Npm-prefixed ID"
    );
}

#[test]
fn promoted_prefixed_values_point_to_prefixed_ids() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/native-core", Ecosystem::Cargo, "native-core");
    write_pkg(root, "packages/native-core", Ecosystem::Npm, "native-core");
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("promotion must succeed");

    let cargo_prefixed = ws
        .graph
        .identity()
        .prefixed
        .get(&(Ecosystem::Cargo, "native-core".to_string()))
        .expect("Cargo prefixed entry must exist");
    let npm_prefixed = ws
        .graph
        .identity()
        .prefixed
        .get(&(Ecosystem::Npm, "native-core".to_string()))
        .expect("Npm prefixed entry must exist");

    assert_eq!(
        cargo_prefixed,
        &PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "native-core".to_string(),
        },
        "index.prefixed value must point to the promoted Cargo-prefixed ID"
    );
    assert_eq!(
        npm_prefixed,
        &PackageId::Prefixed {
            ecosystem: Ecosystem::Npm,
            name: "native-core".to_string(),
        },
        "index.prefixed value must point to the promoted Npm-prefixed ID"
    );
}

/// A platform npm manifest co-located with a Cargo owner whose
/// owner later gets promoted via a disjoint cross-ecosystem bare-name
/// collision elsewhere in the workspace: `index.platform`'s stored
/// owner id must be rewritten to the promoted Prefixed id, matching the
/// same rewrite already applied to `index.bare`/`index.native`/
/// `index.prefixed` for this exact scenario.
#[test]
fn promoted_platform_index_value_points_to_prefixed_owner_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/native-core", Ecosystem::Cargo, "native-core");
    std::fs::write(
        root.join("crates/native-core/package.json"),
        r#"{"name":"@myorg/native-core-linux-x64-gnu","version":"0.1.0","os":["linux"],"cpu":["x64"]}"#,
    )
    .unwrap();
    write_pkg(root, "packages/native-core", Ecosystem::Npm, "native-core");

    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("promotion must succeed");

    let platform_entry = ws
        .graph
        .identity()
        .platform
        .get("@myorg/native-core-linux-x64-gnu")
        .expect("platform entry must exist");

    assert_eq!(
        platform_entry.0,
        PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "native-core".to_string(),
        },
        "index.platform's owner id must point to the promoted Cargo-prefixed id, \
             not the stale pre-promotion bare id"
    );
}

#[test]
fn unpromoted_standalone_cargo_package_retains_bare_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/single", Ecosystem::Cargo, "single");
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws =
        crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("standalone package load must succeed");
    let bare_id = PackageId::Bare("single".to_string());
    assert!(
        ws.graph.get(&bare_id).is_some(),
        "unpromoted single-ecosystem package must register under Bare ID"
    );
    let prefixed_id = PackageId::Prefixed {
        ecosystem: Ecosystem::Cargo,
        name: "single".to_string(),
    };
    assert!(
        ws.graph.get(&prefixed_id).is_none(),
        "unpromoted package must NOT register under Prefixed ID directly"
    );
    assert_eq!(
        ws.graph.identity().bare.get("single"),
        Some(&bare_id),
        "index.bare must point to the Bare ID"
    );
    let resolved = ws
        .graph
        .identity()
        .resolve_human("cargo:single", &[])
        .expect("cargo:single must resolve");
    assert_eq!(
        resolved, bare_id,
        "cargo:single human lookup must resolve to the Bare ID"
    );
}

/// A platform package that IS a workspace member, named by no owner's
/// `optionalDependencies` anywhere: `PlatformPackageWithoutOwner` fires with the
/// member-specific suffix ("released as its own package").
#[test]
fn platform_member_without_any_owner_gets_the_released_as_its_own_package_message() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("packages/orphan-linux-x64-gnu")).unwrap();
    std::fs::write(
        root.join("packages/orphan-linux-x64-gnu/package.json"),
        r#"{"name":"orphan-linux-x64-gnu","version":"0.1.0","os":["linux"],"cpu":["x64"]}"#,
    )
    .unwrap();

    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load must succeed");

    let diag = ws
        .graph
        .diagnostics
        .iter()
        .find(|d| d.code == DiagnosticCode::PlatformPackageWithoutOwner)
        .expect("expected a PlatformPackageWithoutOwner diagnostic");
    assert!(
        diag.message
            .contains("is not named in any npm package's optionalDependencies"),
        "got: {}",
        diag.message
    );
    assert!(
        diag.message
            .contains("it is released as its own package instead of as a platform manifest of its owner"),
        "got: {}",
        diag.message
    );
}

/// A platform candidate OUTSIDE the npm workspace's membership, named by two different
/// admitted packages' `optionalDependencies` (ambiguous, so still ownerless):
/// `PlatformPackageWithoutOwner` fires with the non-member suffix ("not released at all").
#[test]
fn platform_non_member_with_ambiguous_owners_gets_the_not_released_at_all_message() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(root.join("package.json"), r#"{"workspaces": ["packages/*"]}"#).unwrap();
    std::fs::create_dir_all(root.join("packages/first")).unwrap();
    std::fs::write(
        root.join("packages/first/package.json"),
        r#"{"name":"first","version":"0.1.0","optionalDependencies":{"shared-linux-x64-gnu":"0.1.0"}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(root.join("packages/second")).unwrap();
    std::fs::write(
        root.join("packages/second/package.json"),
        r#"{"name":"second","version":"0.1.0","optionalDependencies":{"shared-linux-x64-gnu":"0.1.0"}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(root.join("tools/shared-linux-x64-gnu")).unwrap();
    std::fs::write(
        root.join("tools/shared-linux-x64-gnu/package.json"),
        r#"{"name":"shared-linux-x64-gnu","version":"0.1.0","os":["linux"],"cpu":["x64"]}"#,
    )
    .unwrap();

    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load must succeed");

    let diag = ws
        .graph
        .diagnostics
        .iter()
        .find(|d| d.code == DiagnosticCode::PlatformPackageWithoutOwner)
        .expect("expected a PlatformPackageWithoutOwner diagnostic");
    assert!(
        diag.message
            .contains("is named in the optionalDependencies of 2 packages"),
        "got: {}",
        diag.message
    );
    assert!(
        diag.message
            .contains("it is outside the npm workspace, so it is not released at all"),
        "got: {}",
        diag.message
    );
}

/// An npm platform candidate (`os`+`cpu`) that sits outside the npm workspace's own
/// membership glob, but whose directory is ALSO a real Cargo package (excluded from the
/// Cargo workspace's membership too, so genuinely a separate, undiscovered package) must
/// never be attached as a bare platform manifest onto an unrelated owner -- `by_path` alone
/// (admitted members) doesn't see it, but a Cargo.toml sitting right there means it's a real
/// package directory, not a detached platform blob.
#[test]
fn platform_candidate_beside_a_foreign_cargo_toml_is_never_attached() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(root.join("package.json"), r#"{"workspaces": ["packages/*"]}"#).unwrap();
    std::fs::create_dir_all(root.join("packages/consumer")).unwrap();
    std::fs::write(
        root.join("packages/consumer/package.json"),
        r#"{"name":"consumer","version":"0.1.0","optionalDependencies":{"outsider-linux-x64-gnu":"0.1.0"}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(root.join("tools/outsider")).unwrap();
    std::fs::write(
        root.join("tools/outsider/package.json"),
        r#"{"name":"outsider-linux-x64-gnu","version":"0.1.0","os":["linux"],"cpu":["x64"]}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("tools/outsider/Cargo.toml"),
        "[package]\nname = \"outsider\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();

    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load must succeed");

    assert!(
        !ws.graph.identity().platform.contains_key("outsider-linux-x64-gnu"),
        "a platform candidate beside a foreign Cargo.toml must never be attached, got: {:?}",
        ws.graph.identity().platform
    );
}

/// A `[[package]]` rule naming an ecosystem the target package doesn't actually have (a
/// `pypi:foo` rule when `foo` only exists as a Cargo crate) matches zero real packages --
/// this must surface as a `PackageRuleMatchedNothing` diagnostic, the same as an empty
/// `[[package-set]]` glob, not be silently absorbed.
#[test]
fn package_rule_matching_no_ecosystem_the_package_has_gets_a_matched_nothing_diagnostic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "foo", Ecosystem::Cargo, "foo");
    std::fs::write(
        root.join("callisto.toml"),
        "[[package]]\nmatch = \"pypi/foo\"\npublish-to = [\"npm\"]\n",
    )
    .unwrap();

    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("workspace load must succeed");

    assert!(
        ws.graph
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::PackageRuleMatchedNothing && d.message.contains("pypi/foo")),
        "expected a PackageRuleMatchedNothing diagnostic naming pypi/foo, got: {:?}",
        ws.graph.diagnostics
    );
}

#[test]
fn case_d_single_path_multi_ecosystem_retains_bare_id_and_preserves_platform() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/hybrid", Ecosystem::Cargo, "hybrid");
    std::fs::write(
        root.join("crates/hybrid/package.json"),
        r#"{"name":"@myorg/hybrid-darwin-arm64","version":"0.1.0","os":["darwin"],"cpu":["arm64"]}"#,
    )
    .unwrap();

    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("dual-manifest workspace must load");

    let bare_id = PackageId::Bare("hybrid".to_string());
    assert!(
        ws.graph.get(&bare_id).is_some(),
        "dual-manifest primary package must register under Bare ID"
    );
    let platform_entry = ws
        .graph
        .identity()
        .platform
        .get("@myorg/hybrid-darwin-arm64")
        .expect("platform entry must exist in IdentityIndex.platform");
    assert_eq!(platform_entry.0, bare_id, "platform entry owner must be the Bare ID");
    assert_eq!(platform_entry.1, PathBuf::from("crates/hybrid/package.json"));
}

#[test]
fn promoted_index_prefixed_holds_two_distinct_ids_not_stale_bare() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/native-core", Ecosystem::Cargo, "native-core");
    write_pkg(root, "packages/native-core", Ecosystem::Npm, "native-core");
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("promotion must succeed");
    let cargo_id = ws
        .graph
        .identity()
        .prefixed
        .get(&(Ecosystem::Cargo, "native-core".to_string()))
        .expect("prefixed Cargo entry must exist");
    let npm_id = ws
        .graph
        .identity()
        .prefixed
        .get(&(Ecosystem::Npm, "native-core".to_string()))
        .expect("prefixed Npm entry must exist");
    assert_eq!(
        *cargo_id,
        PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "native-core".to_string()
        }
    );
    assert_eq!(
        *npm_id,
        PackageId::Prefixed {
            ecosystem: Ecosystem::Npm,
            name: "native-core".to_string()
        }
    );
    assert_ne!(
        cargo_id, npm_id,
        "a distinct-id count of 2 is required for ResolvedConfig.promoted_siblings' derivation to retain this name"
    );
}

#[test]
fn tri_fixture_rejects_only_the_offending_member_and_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "aaa/tri", Ecosystem::Cargo, "tri");
    std::fs::write(root.join("aaa/tri/package.json"), r#"{"name":"tri","version":"0.1.0"}"#).unwrap();
    write_pkg(root, "bbb/tri", Ecosystem::Pypi, "tri");
    write_pkg(root, "ccc/tri", Ecosystem::Npm, "tri");
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let err = match crate::Workspace::load(root.to_path_buf(), &locator, &runner) {
        Err(e) => e,
        Ok(_) => panic!("expected DuplicatePackage error, got Ok"),
    };
    match err {
        GraphError::DuplicatePackage { id, paths } => {
            assert_eq!(id, PackageId::Bare("tri".to_string()));
            assert!(paths.contains(&PathBuf::from("aaa/tri")));
            assert!(paths.contains(&PathBuf::from("ccc/tri")));
            assert!(
                !paths.contains(&PathBuf::from("bbb/tri")),
                "the non-offending group member bbb/tri must not be named"
            );
        }
        other => panic!("expected DuplicatePackage, got {other:?}"),
    }
}

#[test]
fn third_path_joins_already_promoted_group() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "aaa", Ecosystem::Cargo, "multi");
    write_pkg(root, "bbb", Ecosystem::Npm, "multi");
    write_pkg(root, "ccc", Ecosystem::Pypi, "multi");
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("N=3 promotion must succeed");
    assert_eq!(ws.graph.packages().count(), 3);
    for eco in [Ecosystem::Cargo, Ecosystem::Npm, Ecosystem::Pypi] {
        let id = PackageId::Prefixed {
            ecosystem: eco,
            name: "multi".to_string(),
        };
        let pkg = ws.graph.get(&id).unwrap_or_else(|| panic!("{eco:?} entry must exist"));
        assert_eq!(
            pkg.manifests.len(),
            1,
            "{eco:?} entry must not bleed in another path's manifest"
        );
    }
}

#[test]
fn third_path_joins_already_promoted_group_reverse_iteration_order() {
    // Same three ecosystems as `third_path_joins_already_promoted_group`, but
    // path names are chosen so BTreeMap's lexicographic by-path iteration
    // visits Pypi first, then Npm, then Cargo -- the reverse ecosystem
    // sequence -- proving the outcome is order-independent.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "path-a-pypi", Ecosystem::Pypi, "multi2");
    write_pkg(root, "path-b-npm", Ecosystem::Npm, "multi2");
    write_pkg(root, "path-c-cargo", Ecosystem::Cargo, "multi2");
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner)
        .expect("N=3 promotion must succeed regardless of by-path iteration order");
    assert_three_promoted_singletons(&ws, "multi2");
}

fn assert_three_promoted_singletons(ws: &crate::Workspace<NoopRunner>, name: &str) {
    assert_eq!(ws.graph.packages().count(), 3);
    for eco in [Ecosystem::Cargo, Ecosystem::Npm, Ecosystem::Pypi] {
        let id = PackageId::Prefixed {
            ecosystem: eco,
            name: name.to_string(),
        };
        let pkg = ws.graph.get(&id).unwrap_or_else(|| panic!("{eco:?} entry must exist"));
        assert_eq!(
            pkg.manifests.len(),
            1,
            "{eco:?} entry must not bleed in another path's manifest"
        );
    }
}

#[test]
fn maturin_pyo3_layout_promotes_cargo_and_pypi_siblings() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "bindings/rust", Ecosystem::Cargo, "mylib");
    write_pkg(root, "bindings/python", Ecosystem::Pypi, "mylib");
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("Cargo/Pypi promotion must succeed");
    let cargo_pkg = ws
        .graph
        .get(&PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "mylib".to_string(),
        })
        .expect("Cargo entry must exist");
    let pypi_pkg = ws
        .graph
        .get(&PackageId::Prefixed {
            ecosystem: Ecosystem::Pypi,
            name: "mylib".to_string(),
        })
        .expect("Pypi entry must exist");
    assert_eq!(cargo_pkg.manifests.len(), 1);
    assert_eq!(cargo_pkg.manifests[0].path, PathBuf::from("bindings/rust/Cargo.toml"));
    assert_eq!(pypi_pkg.manifests.len(), 1);
    assert_eq!(
        pypi_pkg.manifests[0].path,
        PathBuf::from("bindings/python/pyproject.toml")
    );
}

#[test]
fn case_d_package_with_no_collision_stays_bare() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/foo", Ecosystem::Cargo, "foo");
    std::fs::write(
        root.join("crates/foo/package.json"),
        r#"{"name":"@myorg/foo","version":"0.1.0"}"#,
    )
    .unwrap();
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("dual-manifest workspace must load");
    assert_eq!(ws.graph.packages().count(), 1);
    let pkg = ws
        .graph
        .get(&PackageId::Bare("foo".to_string()))
        .expect("single Bare(foo) entry must exist, unpromoted");
    assert_eq!(
        pkg.manifests.len(),
        2,
        "both the Cargo and npm manifest belong to the one dual-manifest package"
    );
}

#[test]
fn npm_consumer_depending_on_cargo_only_name_gets_no_edge_and_diagnostic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/foo", Ecosystem::Cargo, "foo");
    std::fs::create_dir_all(root.join("packages/consumer")).unwrap();
    std::fs::write(
        root.join("packages/consumer/package.json"),
        r#"{"name":"consumer","version":"0.1.0","dependencies":{"foo":"^1.0.0"}}"#,
    )
    .unwrap();
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner)
        .expect("load must succeed even with an unresolved dependency name");
    let cargo_foo = PackageId::Bare("foo".to_string());
    assert!(
        !ws.graph.edges().iter().any(|e| e.to == cargo_foo),
        "no DepEdge must be created to the Cargo foo id from an npm consumer's same-name miss"
    );
    let diag = ws
        .graph
        .diagnostics()
        .iter()
        .find(|d| {
            d.code == callisto_model::DiagnosticCode::UnknownPackage
                && d.severity == callisto_model::DiagnosticSeverity::Warning
                && d.message.contains("foo")
        })
        .expect("an UnknownPackage warning diagnostic naming foo must be present");
    assert!(diag.message.contains("ambiguous") || diag.message.contains("cargo/foo"));
}

#[test]
fn npm_consumer_depending_on_serde_with_cargo_serde_present_gets_no_edge_and_diagnostic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/serde", Ecosystem::Cargo, "serde");
    std::fs::create_dir_all(root.join("packages/consumer")).unwrap();
    std::fs::write(
        root.join("packages/consumer/package.json"),
        r#"{"name":"consumer","version":"0.1.0","dependencies":{"serde":"^1"}}"#,
    )
    .unwrap();
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("load must succeed");
    let cargo_serde = PackageId::Bare("serde".to_string());
    assert!(!ws.graph.edges().iter().any(|e| e.to == cargo_serde));
    let diag = ws
        .graph
        .diagnostics()
        .iter()
        .find(|d| {
            d.code == callisto_model::DiagnosticCode::UnknownPackage
                && d.severity == callisto_model::DiagnosticSeverity::Warning
                && d.message.contains("serde")
        })
        .expect("an UnknownPackage warning diagnostic naming serde must be present");
    assert!(diag.message.contains("ambiguous") || diag.message.contains("cargo/serde"));
}

#[test]
fn npm_consumer_depending_on_ambiguous_lib_with_cargo_and_pypi_present_gets_exactly_one_diagnostic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/ambiguous-lib", Ecosystem::Cargo, "ambiguous-lib");
    write_pkg(root, "py/ambiguous-lib", Ecosystem::Pypi, "ambiguous-lib");
    std::fs::create_dir_all(root.join("packages/consumer")).unwrap();
    std::fs::write(
        root.join("packages/consumer/package.json"),
        r#"{"name":"consumer","version":"0.1.0","dependencies":{"ambiguous-lib":"^1"}}"#,
    )
    .unwrap();
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner).expect("load must succeed");
    let diags: Vec<_> = ws
        .graph
        .diagnostics()
        .iter()
        .filter(|d| d.code == callisto_model::DiagnosticCode::UnknownPackage && d.message.contains("ambiguous-lib"))
        .collect();
    assert_eq!(
        diags.len(),
        1,
        "exactly one diagnostic, not one per candidate ecosystem"
    );
}

#[test]
fn prefixed_package_rules_still_apply_correctly_to_promoted_siblings() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/native-core", Ecosystem::Cargo, "native-core");
    write_pkg(root, "packages/native-core", Ecosystem::Npm, "native-core");
    std::fs::write(
            root.join("callisto.toml"),
            "[[package]]\nmatch = \"cargo/native-core\"\nrelease-trigger = \"changeset\"\n\n[[package]]\nmatch = \"npm/native-core\"\nrelease-trigger = \"auto\"\n",
        )
        .unwrap();
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let runner = NoopRunner;
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &runner)
        .expect("prefixed rules must not trigger the ambiguity check");
    let cargo_pkg = ws
        .graph
        .get(&PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "native-core".to_string(),
        })
        .unwrap();
    let npm_pkg = ws
        .graph
        .get(&PackageId::Prefixed {
            ecosystem: Ecosystem::Npm,
            name: "native-core".to_string(),
        })
        .unwrap();
    assert_eq!(cargo_pkg.release_trigger, ReleaseTrigger::Changeset);
    assert_eq!(npm_pkg.release_trigger, ReleaseTrigger::Auto);
}

/// The mirror of `promoted_platform_index_value_points_to_prefixed_owner_id`: the platform manifest is
/// co-located with the owner that sorts second, so it is the second path's platform keys that are re-keyed.
#[test]
fn promoted_platform_index_value_follows_the_later_path_too() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_pkg(root, "crates/native-core", Ecosystem::Npm, "native-core");
    write_pkg(root, "packages/native-core", Ecosystem::Cargo, "native-core");
    std::fs::write(
        root.join("packages/native-core/package.json"),
        r#"{"name":"@myorg/native-core-linux-x64-gnu","version":"0.1.0","os":["linux"],"cpu":["x64"]}"#,
    )
    .unwrap();

    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &NoopRunner).expect("promotion must succeed");

    let platform_entry = ws
        .graph
        .identity()
        .platform
        .get("@myorg/native-core-linux-x64-gnu")
        .expect("platform entry must exist");
    assert_eq!(
        platform_entry.0,
        PackageId::Prefixed {
            ecosystem: Ecosystem::Cargo,
            name: "native-core".to_string(),
        }
    );
    assert_eq!(
        platform_entry.1,
        std::path::PathBuf::from("packages/native-core/package.json")
    );
}

fn pnpm_workspace(members: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(root.join("pnpm-workspace.yaml"), "packages:\n  - \"packages/*\"\n").unwrap();
    for (name, dependencies) in members {
        std::fs::create_dir_all(root.join("packages").join(name)).unwrap();
        std::fs::write(
            root.join("packages").join(name).join("package.json"),
            format!(r#"{{"name":"{name}","version":"0.1.0"{dependencies}}}"#),
        )
        .unwrap();
    }
    dir
}

/// A `workspace:<relative path>` dependency resolves by the path it names, so the dependency key is only
/// an alias and may be spelled differently from the member's own name.
#[test]
fn a_relative_pnpm_workspace_dependency_resolves_by_path_not_by_key() {
    let dir = pnpm_workspace(&[("a", r#","dependencies":{"bee":"workspace:../b"}"#), ("b", "")]);
    let locator = crate::locate::IgnoreWalkLocator::new(dir.path());
    let ws = crate::Workspace::load(dir.path().to_path_buf(), &locator, &NoopRunner).expect("load");
    let edges: Vec<_> = ws
        .graph
        .edges()
        .iter()
        .map(|edge| {
            (
                edge.from.name().to_owned(),
                edge.to.name().to_owned(),
                edge.name.clone(),
            )
        })
        .collect();
    assert_eq!(edges, [("a".to_owned(), "b".to_owned(), "bee".to_owned())]);
    assert!(
        !ws.graph
            .diagnostics()
            .iter()
            .any(|d| d.code == callisto_model::DiagnosticCode::UnknownPackage),
        "{:?}",
        ws.graph.diagnostics()
    );
}

/// A relative path that names no admitted member makes no edge and says so.
#[test]
fn a_relative_pnpm_workspace_dependency_naming_no_member_is_diagnosed() {
    let dir = pnpm_workspace(&[("a", r#","dependencies":{"ghost":"workspace:../ghost"}"#), ("b", "")]);
    let locator = crate::locate::IgnoreWalkLocator::new(dir.path());
    let ws = crate::Workspace::load(dir.path().to_path_buf(), &locator, &NoopRunner).expect("load");
    assert!(ws.graph.edges().is_empty());
    let diagnostic = ws
        .graph
        .diagnostics()
        .iter()
        .find(|d| d.code == callisto_model::DiagnosticCode::UnknownPackage)
        .expect("an UnknownPackage warning");
    assert!(
        diagnostic
            .message
            .contains("does not identify an admitted npm workspace member"),
        "{}",
        diagnostic.message
    );
    assert_eq!(diagnostic.package.as_ref().map(PackageId::name), Some("a"));
}

/// `workspace = true` on a dependency the workspace root does not declare keeps the member's own spec and
/// still links the packages.
#[test]
fn an_inherited_cargo_dependency_missing_from_the_workspace_table_still_makes_an_edge() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"a\", \"b\"]\n").unwrap();
    for (name, dependencies) in [("a", "[dependencies]\nb = { workspace = true }\n"), ("b", "")] {
        std::fs::create_dir_all(root.join(name)).unwrap();
        std::fs::write(
            root.join(name).join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{dependencies}"),
        )
        .unwrap();
    }
    let locator = crate::locate::IgnoreWalkLocator::new(root);
    let ws = crate::Workspace::load(root.to_path_buf(), &locator, &NoopRunner).expect("load");
    let edges: Vec<_> = ws
        .graph
        .edges()
        .iter()
        .map(|edge| (edge.from.name().to_owned(), edge.to.name().to_owned(), edge.inherited))
        .collect();
    assert_eq!(edges, [("a".to_owned(), "b".to_owned(), true)]);
}

fn npm_package_at(dir: &str) -> callisto_model::Package {
    callisto_model::Package {
        id: PackageId::Bare(dir.rsplit('/').next().unwrap().to_string()),
        manifests: vec![ManifestDecl::new(
            format!("{dir}/package.json"),
            ManifestRole::Canonical,
            callisto_model::ManifestFormat::PackageJson,
        )
        .unwrap()],
        changelog: None,
        release_trigger: callisto_model::ReleaseTrigger::Changeset,
        publish_to: Vec::new(),
        tag_template: None,
    }
}

/// A relative `workspace:` path finds the package at that directory, and a path to nothing finds none.
#[test]
fn a_relative_workspace_path_names_the_package_there_or_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("packages/a")).unwrap();
    std::fs::create_dir_all(root.join("packages/b")).unwrap();
    let packages = [npm_package_at("packages/a"), npm_package_at("packages/b")];
    let find = |relative: &str| {
        relative_workspace_package(root, Path::new("packages/a/package.json"), relative, packages.iter()).unwrap()
    };
    assert_eq!(find("../b"), Some(PackageId::Bare("b".to_string())));
    assert_eq!(find("../missing"), None, "absent is not an error");
    assert_eq!(find("../../.."), None, "a path that leaves the workspace names nothing");
}

/// A directory that cannot be searched is an error, never read as "no such package".
#[cfg(unix)]
#[test]
fn a_relative_workspace_path_that_cannot_be_inspected_is_an_error() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("packages/a")).unwrap();
    std::fs::create_dir_all(root.join("locked/inner")).unwrap();
    std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
    let packages = [npm_package_at("packages/a")];
    let result = relative_workspace_package(
        root,
        Path::new("packages/a/package.json"),
        "../../locked/inner",
        packages.iter(),
    );
    std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
    if std::fs::read_dir(root.join("locked")).is_ok() && result.is_ok() {
        // Privileges that ignore directory modes (root): nothing to observe.
        return;
    }
    assert!(result.is_err(), "{result:?}");
}
