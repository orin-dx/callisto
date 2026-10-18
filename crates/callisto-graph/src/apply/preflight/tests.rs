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

/// Reports that no path is ignored, as `git check-ignore` does.
struct NothingIgnored;

impl callisto_model::CommandRunner for NothingIgnored {
    fn run(
        &self,
        _program: &str,
        _args: &[&str],
        _cwd: &Path,
    ) -> Result<callisto_model::CommandOutput, callisto_model::CommandError> {
        Ok(callisto_model::CommandOutput {
            exit_code: Some(1),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

fn check_plan(root: &Path, plan: &VersionPlan) -> Result<(), Vec<String>> {
    check_plan_with(root, plan, &NothingIgnored)
}

fn check_plan_with<R: callisto_model::CommandRunner>(
    root: &Path,
    plan: &VersionPlan,
    runner: &R,
) -> Result<(), Vec<String>> {
    match check(
        root,
        plan,
        &ApplyOptions::default(),
        &ApplyPermit::force_for_tests(),
        runner,
    ) {
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

fn repo_ignoring(root: &Path, ignore: &str) {
    callisto_fixtures::git::init_repo(root);
    std::fs::write(root.join(".gitignore"), ignore).unwrap();
    callisto_fixtures::git::run_git(root, &["add", ".gitignore"]);
}

/// A manifest a `.gitignore` ignores can be edited but never staged, so the run stops before it writes anything.
#[test]
fn a_target_that_a_gitignore_ignores_is_refused_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    repo_ignoring(root, "hidden/\n");
    std::fs::create_dir_all(root.join("hidden")).unwrap();
    std::fs::write(
        root.join("hidden/Cargo.toml"),
        "[package]\nname = \"h\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"r\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    let problems = check_plan_with(
        root,
        &plan_writing(&["Cargo.toml", "hidden/Cargo.toml"]),
        &callisto_fixtures::git::GitRunner,
    )
    .unwrap_err();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("hidden/Cargo.toml") && problems[0].contains("ignored by a .gitignore"),
        "{problems:?}"
    );
}

/// A file Git already tracks is staged however a `.gitignore` reads, so it is not a problem.
#[test]
fn a_tracked_file_that_a_gitignore_also_matches_is_not_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    repo_ignoring(root, "Cargo.toml\n");
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"r\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    callisto_fixtures::git::run_git(root, &["add", "-f", "Cargo.toml"]);

    check_plan_with(root, &plan_writing(&["Cargo.toml"]), &callisto_fixtures::git::GitRunner).unwrap();
}

/// A failing `git check-ignore` (not a repository, say) stops the run rather than being read as "nothing ignored".
#[test]
fn a_check_ignore_failure_is_an_error_not_an_empty_answer() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"r\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    let error = check(
        dir.path(),
        &plan_writing(&["Cargo.toml"]),
        &ApplyOptions::default(),
        &ApplyPermit::force_for_tests(),
        &callisto_fixtures::git::GitRunner,
    )
    .unwrap_err();
    assert!(matches!(error, GraphError::Command(_)), "{error:?}");
}
