use callisto_model::{ApplyPermit, PackageId, Severity, Version, VersionGrammar};

use super::*;
use crate::plan::{OptionalDepUpdate, PlannedBump};

fn version(raw: &str) -> Version {
    Version::parse(raw, VersionGrammar::SemVer).unwrap()
}

fn bump_writing(paths: &[&str]) -> PlannedBump {
    PlannedBump {
        package: PackageId::parse("cargo:demo").unwrap(),
        from: version("1.0.0"),
        to: version("1.1.0"),
        severity: Severity::Minor,
        governed_by: None,
        reason: None,
        writes: paths
            .iter()
            .map(|path| VersionWriteTarget::Manifest(PathBuf::from(path)))
            .collect(),
    }
}

fn plan_writing(paths: &[&str]) -> VersionPlan {
    VersionPlan {
        bumps: vec![bump_writing(paths)],
        ..VersionPlan::default()
    }
}

fn check_plan(root: &Path, plan: &VersionPlan) -> Result<(), Vec<String>> {
    match check(root, plan, &ApplyOptions::default(), &ApplyPermit::force_for_tests()) {
        Ok(()) => Ok(()),
        Err(GraphError::ApplyPreflight { problems }) => Err(problems),
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn writable_manifests_pass_and_leave_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
    assert!(check_plan(dir.path(), &plan_writing(&["Cargo.toml"])).is_ok());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn every_bad_target_is_named_in_one_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("good.toml"), "x").unwrap();
    std::fs::create_dir(dir.path().join("is-a-dir")).unwrap();
    let problems = check_plan(
        dir.path(),
        &plan_writing(&["good.toml", "missing.toml", "is-a-dir", "../outside.toml"]),
    )
    .unwrap_err();
    assert_eq!(problems.len(), 3, "{problems:?}");
    assert!(problems
        .iter()
        .any(|p| p.contains("missing.toml") && p.contains("does not exist")));
    assert!(problems
        .iter()
        .any(|p| p.contains("is-a-dir") && p.contains("not a regular file")));
    assert!(problems
        .iter()
        .any(|p| p.contains("outside.toml") && p.contains("outside the workspace")));
}

#[cfg(unix)]
#[test]
fn a_manifest_beneath_or_behind_a_symbolic_link_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    std::fs::write(external.path().join("Cargo.toml"), "x").unwrap();
    std::os::unix::fs::symlink(external.path(), dir.path().join("linked")).unwrap();
    std::fs::write(dir.path().join("real.toml"), "x").unwrap();
    std::os::unix::fs::symlink(dir.path().join("real.toml"), dir.path().join("alias.toml")).unwrap();

    let problems = check_plan(dir.path(), &plan_writing(&["linked/Cargo.toml", "alias.toml"])).unwrap_err();
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems.iter().all(|p| p.contains("symbolic link")), "{problems:?}");
}

#[test]
fn a_changelog_may_be_created_but_a_changeset_removal_only_needs_a_clean_path() {
    let dir = tempfile::tempdir().unwrap();
    let plan = VersionPlan {
        consumed_changesets: vec![PathBuf::from(".changeset/gone.md")],
        changelog_writes: vec![crate::plan::ChangelogWrite {
            changelog_path: PathBuf::from("docs/CHANGELOG.md"),
            input: crate::changelog::ChangelogInput {
                package: PackageId::parse("cargo:demo").unwrap(),
                from: version("1.0.0"),
                to: Some(version("1.1.0")),
                entries: Vec::new(),
            },
        }],
        ..VersionPlan::default()
    };
    assert!(check_plan(dir.path(), &plan).is_ok());
    // A transient (snapshot) run writes neither, so neither is checked.
    let transient = ApplyOptions {
        transient: true,
        ..ApplyOptions::default()
    };
    assert!(targets(&plan, &transient).is_empty());
}

#[test]
fn an_unreadable_manifest_directory_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("pkg");
    std::fs::create_dir(&pkg).unwrap();
    std::fs::write(pkg.join("Cargo.toml"), "x").unwrap();
    let Some(_locked) = callisto_fixtures::locked_dir::LockedDir::lock(&pkg) else {
        return;
    };
    let problems = check_plan(dir.path(), &plan_writing(&["pkg/Cargo.toml"])).unwrap_err();
    assert_eq!(problems.len(), 1, "{problems:?}");
}

#[test]
fn optional_dependency_and_platform_writes_are_checked_too() {
    let dir = tempfile::tempdir().unwrap();
    let plan = VersionPlan {
        optional_dep_updates: vec![OptionalDepUpdate {
            manifest: PathBuf::from("missing/package.json"),
            updates: Vec::new(),
        }],
        ..VersionPlan::default()
    };
    let problems = check_plan(dir.path(), &plan).unwrap_err();
    assert!(problems[0].contains("missing/package.json"), "{problems:?}");
}
