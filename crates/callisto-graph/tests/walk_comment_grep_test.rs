#[test]
fn no_stale_always_bare_comment_remains() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/walk.rs")).unwrap();
    assert!(
        !src.contains("`id` itself is always PackageId::Bare here"),
        "stale comment at the package_ecosystems computation must be corrected"
    );
    assert!(
        !src.contains("Packages-map keys are ALWAYS PackageId::Bare"),
        "stale comment above the SPEC-002 AC-5 diagnostic loop must be corrected"
    );
}
