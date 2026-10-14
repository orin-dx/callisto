use std::path::Path;

use callisto_model::{CommandError, CommandOutput};

use super::*;
use crate::commands::release::tests::RealGitRunner;
use crate::IgnoreWalkLocator;

const ORIGIN: &str = "https://github.com/example/tools.git";

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn repo(files: &[(&str, &str)], origin: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    callisto_fixtures::git::init_repo(dir.path());
    if let Some(origin) = origin {
        git(dir.path(), &["remote", "add", "origin", origin]);
    }
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "fixture"]);
    dir
}

fn tag(root: &Path, name: &str) {
    git(root, &["tag", name]);
}

fn cargo(name: &str, version: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n")
}

/// A Cargo binary `app`, a Cargo library `core`, and an npm library `web`.
fn workspace() -> tempfile::TempDir {
    repo(
        &[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"app\", \"core\"]\nresolver = \"2\"\n",
            ),
            ("app/Cargo.toml", &cargo("app", "1.0.0")),
            ("app/src/main.rs", "fn main() {}\n"),
            ("core/Cargo.toml", &cargo("core", "1.0.0")),
            ("core/src/lib.rs", "\n"),
            ("web/package.json", r#"{"name":"web","version":"2.0.0"}"#),
        ],
        Some(ORIGIN),
    )
}

fn detect_in(root: &Path) -> Result<InitFacts, GraphError> {
    detect(root, &IgnoreWalkLocator::new(root), &RealGitRunner)
}

fn package<'a>(facts: &'a InitFacts, id: &str) -> &'a InitPackage {
    facts.packages.iter().find(|package| package.id == id).unwrap()
}

fn release(targets: &[&str]) -> InitAnswers {
    InitAnswers {
        versioning: Versioning::Independent,
        binaries: Some(BinaryRelease {
            product: "cargo/app".to_owned(),
            forge_repository: GitHubRepository::parse("example/tools").unwrap(),
            targets: targets.iter().map(|target| (*target).to_owned()).collect(),
        }),
    }
}

const INDEPENDENT: InitAnswers = InitAnswers {
    versioning: Versioning::Independent,
    binaries: None,
};

// Facts report ecosystems, packages, origin, tags, and binaries.
#[test]
fn detect_reports_workspace_facts() {
    let dir = workspace();
    tag(dir.path(), "app@0.9.0");
    let facts = detect_in(dir.path()).unwrap();
    assert_eq!(facts.ecosystems, BTreeSet::from([Ecosystem::Cargo, Ecosystem::Npm]));
    let ids: Vec<&str> = facts.packages.iter().map(|package| package.id.as_str()).collect();
    assert_eq!(ids, ["cargo/app", "cargo/core", "npm/web"]);
    assert_eq!(facts.origin.as_deref(), Some(ORIGIN));
    assert!(facts.has_commit);
    assert_eq!(facts.origin_repository.as_ref().unwrap().as_slug(), "example/tools");
    assert_eq!(
        package(&facts, "cargo/app").last_tag.as_ref().map(TagName::as_str),
        Some("app@0.9.0")
    );
    assert_eq!(package(&facts, "cargo/core").last_tag, None);
    assert_eq!(package(&facts, "cargo/app").bin_names, ["app"]);
    assert_eq!(package(&facts, "cargo/app").cargo_bin_names, ["app"]);
    let binaries: Vec<&str> = facts.binary_packages().map(|package| package.id.as_str()).collect();
    assert_eq!(binaries, ["cargo/app"]);
}

// Registry executables are detected but are not supported artifact builders.
#[test]
fn npm_bin_and_pypi_scripts_are_not_artifact_builders() {
    let dir = repo(
        &[
            (
                "cli/package.json",
                r#"{"name":"cli","version":"1.0.0","bin":{"cli":"cli.js"}}"#,
            ),
            (
                "py/pyproject.toml",
                "[project]\nname = \"pytool\"\nversion = \"1.0.0\"\n\n[project.scripts]\npytool = \"pytool:main\"\n",
            ),
            (
                "lib/pyproject.toml",
                "[project]\nname = \"pylib\"\nversion = \"1.0.0\"\n",
            ),
        ],
        Some(ORIGIN),
    );
    let facts = detect_in(dir.path()).unwrap();
    let binaries: Vec<&str> = facts.binary_packages().map(|package| package.id.as_str()).collect();
    let unsupported: Vec<&str> = facts
        .unsupported_artifact_packages()
        .map(|package| package.id.as_str())
        .collect();
    assert!(binaries.is_empty());
    assert_eq!(unsupported, ["npm/cli", "pypi/pytool"]);
}

#[test]
fn npm_bin_does_not_make_a_cargo_library_an_artifact_builder() {
    let dir = repo(
        &[
            (
                "my-pkg/Cargo.toml",
                "[package]\nname = \"my-pkg\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
            ),
            ("my-pkg/src/lib.rs", "pub fn library() {}\n"),
            (
                "my-pkg/package.json",
                r#"{"name":"my-pkg","version":"1.0.0","bin":{"my-pkg":"cli.js"}}"#,
            ),
        ],
        Some(ORIGIN),
    );
    let facts = detect_in(dir.path()).unwrap();
    let package = package(&facts, "cargo/my-pkg");

    assert_eq!(package.bin_names, ["my-pkg"]);
    assert!(package.cargo_bin_names.is_empty());
    assert_eq!(package.unsupported_bin_ecosystems, ["npm"]);
    assert!(facts.binary_packages().next().is_none());
    assert_eq!(facts.unsupported_artifact_packages().next().unwrap().id, "cargo/my-pkg");
    assert!(matches!(
        facts.product_package("cargo/my-pkg"),
        Err(GraphError::InitUnsupportedArtifactBuilder { ecosystem, .. }) if ecosystem == "npm"
    ));
}

// An existing config is an error, and its [init] table is never read.
#[test]
fn existing_config_is_already_initialized() {
    let dir = workspace();
    let config = "[init]\necosystems = [\"cargo\"]\n";
    std::fs::write(dir.path().join("callisto.toml"), config).unwrap();
    let error = detect_in(dir.path()).unwrap_err();
    assert!(matches!(error, GraphError::InitAlreadyInitialized { .. }), "{error}");
    assert!(error.to_string().contains("already initialized"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("callisto.toml")).unwrap(),
        config
    );
    assert!(crate::config::load(dir.path()).is_ok(), "[init] still parses");
    let permit = ApplyPermit::force_for_tests();
    assert!(matches!(
        write(dir.path(), empty_config(), &permit).unwrap_err(),
        GraphError::InitAlreadyInitialized { .. }
    ));
}

/// A write failure during `init` names the exact file it failed to write
/// (`GraphError::InitIo`, E278), mirroring `apply.rs`'s path-carrying
/// `GraphError::ApplyIo` (E122) rather than a pathless generic error.
#[test]
fn write_changeset_readme_names_the_failing_path() {
    let dir = tempfile::tempdir().unwrap();
    // A file (not a directory) at `.changeset` makes `create_dir_all` fail.
    std::fs::write(dir.path().join(".changeset"), "not a directory").unwrap();
    let permit = ApplyPermit::force_for_tests();

    let error = write_changeset_readme(dir.path(), &permit).unwrap_err();

    let expected_path = dir.path().join(".changeset");
    assert!(
        matches!(&error, GraphError::InitIo { path, .. } if path == &expected_path),
        "expected InitIo naming {}, got {error:?}",
        expected_path.display()
    );
    assert_eq!(
        miette::Diagnostic::code(&error).map(|c| c.to_string()),
        Some("E278".to_string())
    );
}

#[test]
fn outside_git_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), cargo("app", "1.0.0")).unwrap();
    let error = detect_in(dir.path()).unwrap_err();
    assert!(matches!(error, GraphError::InitNotGitRepository { .. }), "{error}");
    assert!(error.to_string().contains("not a Git repository"));
}

// No `origin` (zero remotes, or only other remotes) detects successfully, with `origin` and
// `origin_repository` unset; the caller decides how to proceed without one.
#[test]
fn missing_origin_detects_with_origin_unset() {
    let files = [("Cargo.toml", cargo("app", "1.0.0"))];
    let files: Vec<(&str, &str)> = files.iter().map(|(path, body)| (*path, body.as_str())).collect();
    let none = repo(&files, None);
    let other = repo(&files, None);
    git(other.path(), &["remote", "add", "upstream", ORIGIN]);
    git(other.path(), &["remote", "add", "fork", ORIGIN]);
    for dir in [none, other] {
        let facts = detect_in(dir.path()).unwrap();
        assert_eq!(facts.origin, None);
        assert_eq!(facts.origin_repository, None);
    }
}

// A rejected origin URL shows the rejection; an accepted non-GitHub one binds.
#[test]
fn origin_url_is_canonicalized() {
    let files = [("Cargo.toml", cargo("app", "1.0.0"))];
    let files: Vec<(&str, &str)> = files.iter().map(|(path, body)| (*path, body.as_str())).collect();
    let rejected = repo(&files, Some("https://user:secret@github.com/example/tools.git"));
    let error = detect_in(rejected.path()).unwrap_err();
    assert!(
        matches!(error, GraphError::UnsafeGitRemote { reason } if reason.contains("credentials")),
        "{error}"
    );

    let gitlab = repo(&files, Some("git@gitlab.com:example/tools.git"));
    let facts = detect_in(gitlab.path()).unwrap();
    assert_eq!(facts.origin.as_deref(), Some("ssh://git@gitlab.com/example/tools.git"));
    assert_eq!(facts.origin_repository, None);
}

// Exactly one non-default convention is written; the default never is.
#[test]
fn a_single_non_default_convention_becomes_a_tag_template() {
    let dir = workspace();
    tag(dir.path(), "app-v1.0.0");
    tag(dir.path(), "app-v0.9.0");
    tag(dir.path(), "core@1.0.0");
    tag(dir.path(), "web-2.0.0");
    let facts = detect_in(dir.path()).unwrap();
    assert_eq!(
        facts.tag_templates,
        BTreeMap::from([
            ("cargo/app".to_owned(), "app-v{version}".to_owned()),
            ("npm/web".to_owned(), "web-{version}".to_owned()),
        ])
    );
    assert_eq!(
        package(&facts, "cargo/app").last_tag.as_ref().map(TagName::as_str),
        Some("app-v1.0.0")
    );
    let config = render_config(&facts, &INDEPENDENT);
    assert!(config.contains("[[package]]\nmatch = \"cargo/app\"\ntag-template = \"app-v{version}\"\n"));
    assert!(!config.contains("core"), "default convention is not written:\n{config}");
}

// A sole `v{version}` package gets that template.
#[test]
fn a_sole_v_tag_package_gets_the_v_template() {
    let single = repo(&[("Cargo.toml", &cargo("app", "1.0.0"))], Some(ORIGIN));
    tag(single.path(), "v1.0.0");
    let facts = detect_in(single.path()).unwrap();
    assert_eq!(
        facts.tag_templates,
        BTreeMap::from([("cargo/app".to_owned(), "v{version}".to_owned())])
    );
}

#[test]
fn several_conventions_for_one_package_are_ambiguous() {
    let dir = workspace();
    tag(dir.path(), "app@1.0.0");
    tag(dir.path(), "app-v1.1.0");
    let error = detect_in(dir.path()).unwrap_err();
    let GraphError::InitTagTemplateAmbiguous { package, matches } = &error else {
        panic!("{error}");
    };
    assert_eq!(package, "cargo/app");
    assert_eq!(
        matches,
        &["`app@{version}` (app@1.0.0)", "`app-v{version}` (app-v1.1.0)"]
    );
    let help = miette::Diagnostic::help(&error).unwrap().to_string();
    assert!(help.contains("tag-template") && help.contains("previous-tag-templates"));
}

#[test]
fn a_v_tag_shared_by_several_packages_is_an_error() {
    let dir = workspace();
    tag(dir.path(), "v1.0.0");
    let error = detect_in(dir.path()).unwrap_err();
    let GraphError::InitSharedVersionTag { packages } = &error else {
        panic!("{error}");
    };
    assert_eq!(packages, &["cargo/app", "cargo/core", "npm/web"]);
}

fn facts_for_render() -> InitFacts {
    let package = |id: &str, bins: &[&str], platform| InitPackage {
        id: id.to_owned(),
        version: Version::parse("1.0.0", callisto_model::VersionGrammar::SemVer).unwrap(),
        bin_names: bins.iter().map(|bin| (*bin).to_owned()).collect(),
        cargo_bin_names: if id.starts_with("cargo/") {
            bins.iter().map(|bin| (*bin).to_owned()).collect()
        } else {
            Vec::new()
        },
        unsupported_bin_ecosystems: if id.starts_with("cargo/") || bins.is_empty() {
            Vec::new()
        } else {
            vec![id.split_once('/').unwrap().0.to_owned()]
        },
        last_tag: None,
        publish_to: vec!["crates-io".to_owned()],
        platform,
    };
    InitFacts {
        root: PathBuf::from("/ws"),
        ecosystems: BTreeSet::from([Ecosystem::Cargo, Ecosystem::Npm]),
        packages: vec![
            package("cargo/app", &["app", "appctl"], false),
            package("cargo/core", &[], false),
            package("npm/web-linux-x64-gnu", &[], true),
        ],
        origin: Some(ORIGIN.to_owned()),
        origin_repository: GitHubRepository::parse("example/tools").ok(),
        tag_templates: BTreeMap::new(),
        has_commit: true,
        has_platform_packages: true,
        has_napi_targets: true,
        has_maturin_targets: false,
    }
}

fn parse(config: &str) -> toml::Table {
    toml::from_str(config).unwrap()
}

#[test]
fn fixed_versioning_writes_one_all_group_of_qualified_non_platform_members() {
    let facts = facts_for_render();
    let fixed = parse(&render_config(
        &facts,
        &InitAnswers {
            versioning: Versioning::Fixed,
            binaries: None,
        },
    ));
    let groups = fixed["fixed-group"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["name"].as_str(), Some("all"));
    assert_eq!(
        groups[0]["members"],
        toml::Value::Array(vec!["cargo/app".into(), "cargo/core".into()])
    );
    let keys: Vec<&String> = fixed.keys().collect();
    assert_eq!(keys, ["fixed-group"]);

    let independent = render_config(&facts, &INDEPENDENT);
    assert_eq!(independent, "# callisto configuration\n");
    assert!(parse(&independent).is_empty());
}

// One artifact per target per bin; no profiles or registry-routes.
#[test]
fn shipping_binaries_writes_release_and_artifacts() {
    let config = render_config(
        &facts_for_render(),
        &release(&["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"]),
    );
    let doc = parse(&config);
    let release_table = doc["release"].as_table().unwrap();
    assert_eq!(release_table["product-package"].as_str(), Some("cargo/app"));
    assert_eq!(release_table["forge-repository"].as_str(), Some("example/tools"));
    let assets: Vec<(&str, &str, &str)> = release_table["artifact"]
        .as_array()
        .unwrap()
        .iter()
        .map(|artifact| {
            (
                artifact["package"].as_str().unwrap(),
                artifact["target"].as_str().unwrap(),
                artifact["asset-name"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        assets,
        [
            (
                "cargo/app",
                "x86_64-unknown-linux-gnu",
                "app-x86_64-unknown-linux-gnu.tar.gz"
            ),
            (
                "cargo/app",
                "x86_64-unknown-linux-gnu",
                "appctl-x86_64-unknown-linux-gnu.tar.gz"
            ),
            ("cargo/app", "x86_64-pc-windows-msvc", "app-x86_64-pc-windows-msvc.zip"),
            (
                "cargo/app",
                "x86_64-pc-windows-msvc",
                "appctl-x86_64-pc-windows-msvc.zip"
            ),
        ]
    );
    let rules = doc["package"].as_array().unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["match"].as_str(), Some("cargo/app"));
    assert_eq!(
        rules[0]["publish-to"],
        toml::Value::Array(vec!["crates-io".into(), "github-release".into()])
    );
    assert!(!config.contains("profiles") && !config.contains("registry-routes"));
    assert!(resolve_config_text(Path::new("/ws"), &config).is_ok());
}

// The preview is the release plan for the in-memory config, excluding tagged versions.
#[test]
fn preview_matches_release_dry_run_without_writing() {
    let dir = workspace();
    tag(dir.path(), "core@1.0.0");
    let facts = detect_in(dir.path()).unwrap();
    let config = render_config(&facts, &INDEPENDENT);
    let locator = IgnoreWalkLocator::new(dir.path());
    let preview = preview(&facts, &config, &locator, &RealGitRunner).unwrap().unwrap();
    assert!(!dir.path().join("callisto.toml").exists());
    let released: Vec<String> = preview
        .intent
        .decision
        .entries
        .iter()
        .map(|entry| entry.package.to_string())
        .collect();
    assert!(!released.iter().any(|package| package.contains("core")), "{released:?}");
    assert!(released.iter().any(|package| package.contains("app")), "{released:?}");

    std::fs::write(dir.path().join("callisto.toml"), &config).unwrap();
    let dry_run =
        crate::commands::plan_local_release(dir.path(), &locator, &RealGitRunner, &[], LocalReleaseSource::Preview)
            .unwrap()
            .unwrap();
    assert_eq!(preview, dry_run);
}

// A binary release previews through the same plan function.
#[test]
fn preview_includes_artifact_uploads() {
    let dir = workspace();
    let facts = detect_in(dir.path()).unwrap();
    let config = render_config(&facts, &release(&["x86_64-unknown-linux-gnu"]));
    let plan = preview(&facts, &config, &IgnoreWalkLocator::new(dir.path()), &RealGitRunner)
        .unwrap()
        .unwrap();
    assert!(plan.intent.operations.iter().any(|operation| matches!(
        operation.id().role,
        callisto_model::ReleaseOperationRole::ArtifactUpload { .. }
    )));
}

#[test]
fn forge_repository_must_be_origins_owner_repo() {
    let facts = facts_for_render();
    assert_eq!(
        forge_repository(&facts, "Example/Tools").unwrap().as_slug(),
        "example/tools"
    );
    let error = forge_repository(&facts, "not a repo").unwrap_err();
    assert!(error.to_string().contains("`not a repo`"), "{error}");

    let error = forge_repository(&facts, "other/tools").unwrap_err();
    assert!(
        matches!(&error, GraphError::InitForgeRepositoryMismatch { configured, origin }
                if configured == "other/tools" && origin == ORIGIN),
        "{error}"
    );
    assert!(error.to_string().contains("`other/tools`") && error.to_string().contains(ORIGIN));
    let help = miette::Diagnostic::help(&error).unwrap().to_string();
    assert!(help.contains("release plan requires"), "{help}");

    let non_github = InitFacts {
        origin_repository: None,
        ..facts
    };
    assert!(matches!(
        forge_repository(&non_github, "example/tools").unwrap_err(),
        GraphError::InitForgeRepositoryMismatch { .. }
    ));
}

#[test]
fn targets_must_be_distinct_and_non_empty() {
    let targets = |values: &[&str]| values.iter().map(|value| (*value).to_owned()).collect::<Vec<_>>();
    assert!(validate_targets(&targets(&["x86_64-unknown-linux-gnu", "made-up-triple"])).is_ok());
    for (bad, offending) in [
        (
            targets(&["x86_64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"]),
            "x86_64-unknown-linux-gnu",
        ),
        (targets(&[""]), ""),
    ] {
        let error = validate_targets(&bad).unwrap_err();
        assert!(
            matches!(&error, GraphError::InitInvalidTargetTriple { triple, .. } if triple == offending),
            "{error}"
        );
    }
}

#[test]
fn product_package_must_be_a_binary_workspace_package() {
    let facts = facts_for_render();
    assert_eq!(facts.product_package("app").unwrap().id, "cargo/app");
    assert_eq!(facts.product_package("cargo/app").unwrap().id, "cargo/app");
    for name in ["cargo/core", "missing"] {
        let error = facts.product_package(name).unwrap_err();
        assert!(
            matches!(&error, GraphError::InitInvalidProductPackage { package, .. } if package == name),
            "{error}"
        );
    }
}

#[test]
fn write_creates_the_readme_only_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let permit = ApplyPermit::force_for_tests();
    let report = write(dir.path(), empty_config(), &permit).unwrap();
    assert!(report.initialized);
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".changeset/README.md")).unwrap(),
        CHANGESET_README
    );

    let kept = tempfile::tempdir().unwrap();
    std::fs::create_dir(kept.path().join(".changeset")).unwrap();
    std::fs::write(kept.path().join(".changeset/README.md"), "mine\n").unwrap();
    write(kept.path(), empty_config(), &permit).unwrap();
    assert_eq!(
        std::fs::read_to_string(kept.path().join(".changeset/README.md")).unwrap(),
        "mine\n"
    );
}

/// A cargo-only, non-platform workspace: `app`/`appctl` binaries plus library `core`.
fn cargo_only_facts() -> InitFacts {
    InitFacts {
        ecosystems: BTreeSet::from([Ecosystem::Cargo]),
        packages: facts_for_render()
            .packages
            .into_iter()
            .filter(|package| !package.platform)
            .collect(),
        has_platform_packages: false,
        has_napi_targets: false,
        ..facts_for_render()
    }
}

const FAKE_COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn render_workflow_0_8_0(facts: &InitFacts, branch: &str) -> String {
    render_workflow(facts, WorkflowShape::Simple, branch, FAKE_COMMIT, "0.8.0")
}

// A non-`main` default branch must reach both the push trigger and the release-PR
// action's own `branch:` input, in both workflow shapes -- callisto-action defaults
// that input to `main`, so a trunk repo without it opens its PR against the wrong base.
#[test]
fn render_workflow_passes_the_detected_branch_to_the_release_pr_action() {
    let simple = render_workflow_0_8_0(&cargo_only_facts(), "trunk");
    assert!(simple.contains("branches: [trunk]"), "{simple}");
    assert!(simple.contains("with: {mode: version-pr, branch: trunk}"), "{simple}");

    let matrix = render_workflow(
        &facts_for_render(),
        WorkflowShape::BuildMatrix,
        "trunk",
        FAKE_COMMIT,
        "0.8.0",
    );
    assert!(matrix.contains("branches: [trunk]"), "{matrix}");
    assert!(matrix.contains("with: {mode: version-pr, branch: trunk}"), "{matrix}");
}

// Two jobs, no matrix/recovery/SHA-gating machinery,
// per-job permissions, and secrets scoped to the cargo-only workspace's own ecosystem.
#[test]
fn render_workflow_is_a_cargo_only_two_job_workflow_within_the_line_budget() {
    let facts = cargo_only_facts();
    let workflow = render_workflow_0_8_0(&facts, "main");
    let line_count = workflow.lines().count();
    assert!(line_count <= 40, "{line_count} lines:\n{workflow}");
    assert_eq!(workflow.matches("runs-on: ubuntu-latest").count(), 2, "{workflow}");
    assert!(workflow.contains("branches: [main]"), "{workflow}");
    assert!(
        workflow.contains("with: {mode: version-pr, branch: main}"),
        "{workflow}"
    );
    assert!(workflow.contains("with: {mode: release}"), "{workflow}");
    assert!(
        workflow.contains(&format!(
            "uses: orin-dx/callisto/.github/actions/callisto-action@{FAKE_COMMIT} # callisto@0.8.0"
        )),
        "{workflow}"
    );
    assert!(
        workflow.contains("permissions: {contents: write, pull-requests: write}"),
        "{workflow}"
    );
    assert!(
        workflow.contains("permissions: {contents: write, id-token: write}"),
        "{workflow}"
    );
    assert!(
        workflow.contains("CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}"),
        "{workflow}"
    );
    assert!(!workflow.contains("NPM_TOKEN"), "{workflow}");
    assert!(!workflow.contains("NPM_CONFIG_PROVENANCE"), "{workflow}");
    assert!(!workflow.contains("PYPI_TOKEN"), "{workflow}");
    for absent in ["needs:", "recovery", "coordinator", "github.sha", "release_source_sha"] {
        assert!(!workflow.contains(absent), "`{absent}` present:\n{workflow}");
    }
}

// A workspace with no cargo/npm/pypi ecosystem wires no ecosystem secret.
#[test]
fn render_workflow_wires_a_secret_per_detected_ecosystem() {
    let mut facts = facts_for_render();
    facts.ecosystems = BTreeSet::from([Ecosystem::Cargo, Ecosystem::Npm, Ecosystem::Pypi]);
    let all = render_workflow_0_8_0(&facts, "main");
    let release = job(&all, "release");
    let version_pr = job(&all, "version-pr");
    for secret in ["CARGO_REGISTRY_TOKEN", "NPM_TOKEN", "PYPI_TOKEN"] {
        assert!(release.contains(secret), "{release}");
        assert!(!version_pr.contains(secret), "{version_pr}");
    }
    assert!(
        release.contains("          NPM_CONFIG_PROVENANCE: \"true\"\n"),
        "{release}"
    );
    assert!(!version_pr.contains("NPM_CONFIG_PROVENANCE"), "{version_pr}");
    assert!(all.lines().count() <= 40, "{}", all.lines().count());

    facts.ecosystems = BTreeSet::from([Ecosystem::Go]);
    let none = render_workflow_0_8_0(&facts, "main");
    for secret in [
        "CARGO_REGISTRY_TOKEN",
        "NPM_TOKEN",
        "PYPI_TOKEN",
        "NPM_CONFIG_PROVENANCE",
    ] {
        assert!(!none.contains(secret), "{none}");
    }
    assert!(none.contains("GH_TOKEN: ${{ github.token }}"), "{none}");
}

// Validates the generated workflow's YAML/GHA-schema well-formedness
// (actionlint) and security posture (zizmor); skips a check whose tool
// isn't installed rather than failing on an environment gap.
#[test]
fn render_workflow_passes_actionlint_and_zizmor_when_available() {
    let mut facts = cargo_only_facts();
    facts.ecosystems = BTreeSet::from([Ecosystem::Cargo, Ecosystem::Npm, Ecosystem::Pypi]);
    lint_workflow(&render_workflow_0_8_0(&facts, "main"));
    lint_workflow(&render_matrix_workflow());
}

fn lint_workflow(workflow: &str) {
    let dir = tempfile::tempdir().unwrap();
    let workflows_dir = dir.path().join(".github/workflows");
    std::fs::create_dir_all(&workflows_dir).unwrap();
    let path = workflows_dir.join("callisto-release.yml");
    std::fs::write(&path, workflow).unwrap();

    match std::process::Command::new("actionlint").arg(&path).output() {
        Ok(output) => assert!(
            output.status.success(),
            "actionlint:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(_) => eprintln!("actionlint not installed; skipping that check"),
    }
    match std::process::Command::new("zizmor")
        .args(["--offline", path.to_str().unwrap()])
        .output()
    {
        Ok(output) => assert!(
            output.status.success(),
            "zizmor:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(_) => eprintln!("zizmor not installed; skipping that check"),
    }
}

/// oxc-react-codegen's shape: a cargo CLI shipping binaries, plus a napi addon with platform packages.
fn oxc_shaped_facts() -> InitFacts {
    let package = |id: &str, bins: &[&str]| InitPackage {
        id: id.to_owned(),
        version: Version::parse("0.1.0", callisto_model::VersionGrammar::SemVer).unwrap(),
        bin_names: bins.iter().map(|bin| (*bin).to_owned()).collect(),
        cargo_bin_names: bins.iter().map(|bin| (*bin).to_owned()).collect(),
        unsupported_bin_ecosystems: Vec::new(),
        last_tag: None,
        publish_to: Vec::new(),
        platform: false,
    };
    InitFacts {
        ecosystems: BTreeSet::from([Ecosystem::Cargo, Ecosystem::Npm]),
        packages: vec![
            package("cargo/oxc-react-docgen", &["oxc-react-docgen"]),
            package("cargo/oxc-react-docgen-core", &[]),
            package("cargo/oxc-react-docgen-napi", &[]),
            package("npm/@oxc-react-docgen/cli", &[]),
            package("npm/@oxc-react-docgen/napi", &[]),
            package("npm/@oxc-react-docgen/vite-plugin", &[]),
        ],
        ..facts_for_render()
    }
}

fn render_matrix_workflow() -> String {
    render_workflow(
        &oxc_shaped_facts(),
        WorkflowShape::BuildMatrix,
        "main",
        FAKE_COMMIT,
        "0.8.0",
    )
}

/// The text of `job`'s block: from its `  <job>:` line to the next job's.
fn job<'a>(workflow: &'a str, job: &str) -> &'a str {
    let start = workflow
        .find(&format!("\n  {job}:\n"))
        .unwrap_or_else(|| panic!("no job `{job}`"))
        + 1;
    let rest = &workflow[start..];
    let end = rest
        .match_indices("\n  ")
        .find(|(index, _)| !rest[index + 3..].starts_with(' '))
        .map_or(rest.len(), |(index, _)| index);
    &rest[..end]
}

// The plan job runs `callisto release plan`
// and hands the intent to the later jobs.
#[test]
fn matrix_workflow_plan_job_runs_release_plan() {
    let workflow = render_matrix_workflow();
    let plan = job(&workflow, "plan");
    assert!(
        plan.contains(
            "callisto release plan --from-release-commit \"$GITHUB_SHA\" --decision .callisto/release-decision.json"
        ),
        "{plan}"
    );
    assert!(
        plan.contains("--out \"$RUNNER_TEMP/release-intent/release-intent.json\""),
        "{plan}"
    );
    assert!(plan.contains("name: release-intent"), "{plan}");
    assert!(plan.contains(&format!(
        "uses: orin-dx/callisto/.github/actions/setup-callisto@{FAKE_COMMIT} # callisto@0.8.0"
    )));
}

// Napi entries flatten `callisto matrix`; each job runs on its hostRunner and
// passes --use-cross when useCross.
#[test]
fn matrix_workflow_build_matrix_flattens_napi_targets() {
    let workflow = render_matrix_workflow();
    assert!(
        job(&workflow, "plan")
            .contains(r#"[.platformTargets[] | select(.kind=="napi") | .targets[] | {kind: "napi"} + .]"#),
        "{workflow}"
    );
    let build = job(&workflow, "build");
    assert!(
        build.contains("include: ${{ fromJSON(needs.plan.outputs.matrix) }}"),
        "{build}"
    );
    assert!(build.contains("runs-on: ${{ matrix.hostRunner }}"), "{build}");
    assert!(build.contains("USE_CROSS: ${{ matrix.useCross }}"), "{build}");
    assert!(
        build.contains(r#"if [[ "$USE_CROSS" == true ]]; then args+=(--use-cross); fi"#),
        "{build}"
    );
}

// A napi entry runs `napi build --platform --release --target` and uploads its output.
#[test]
fn matrix_workflow_napi_entry_builds_and_uploads() {
    let build = job(&render_matrix_workflow(), "build").to_owned();
    assert!(
        build.contains(r#"args=(--platform --release --target "$TRIPLE" --output-dir "$RUNNER_TEMP/napi-out")"#),
        "{build}"
    );
    assert!(
        build.contains(r#"npx --yes --package @napi-rs/cli@3.10.4 napi build "${args[@]}""#),
        "{build}"
    );
    assert!(build.contains("MANIFEST_PATH: ${{ matrix.manifestPath }}"), "{build}");
    assert!(
        build.contains(r#"args+=(--manifest-path "$GITHUB_WORKSPACE/$MANIFEST_PATH")"#),
        "{build}"
    );
    assert!(build.contains("name: napi-${{ matrix.artifactName }}"), "{build}");
    assert!(build.contains("path: ${{ runner.temp }}/napi-out/*.node"), "{build}");
}

// One entry per intent artifact slot, built with `cargo build --release --target`
// and archived to the slot's asset name.
#[test]
fn matrix_workflow_artifact_slots_build_and_archive_to_the_asset_name() {
    let workflow = render_matrix_workflow();
    let plan = job(&workflow, "plan");
    assert!(plan.contains("[.artifactSlots[] as $slot"), "{plan}");
    assert!(
        plan.contains(r#"select(.artifactName == $slot.assetName and .triple == $slot.platform)"#),
        "{plan}"
    );
    assert!(
        plan.contains("callisto matrix has no entry for artifact slot"),
        "{plan}"
    );
    let build = job(&workflow, "build");
    assert!(
        build.contains(r#""$cargo_bin" build --release --locked --target "$TRIPLE" --package "$PACKAGE""#),
        "{build}"
    );
    assert!(build.contains(r#"tar -czf "$out/$ASSET" ./*"#), "{build}");
    assert!(build.contains("actions/attest-build-provenance@"), "{build}");
    assert!(build.contains("name: asset-${{ matrix.artifactName }}"), "{build}");
}

// Napi artifacts are placed after every build, then the execute job
// runs `release artifact-manifest` before `release execute`.
#[test]
fn matrix_workflow_execute_places_napi_artifacts_then_manifests_then_executes() {
    let workflow = render_matrix_workflow();
    let execute = job(&workflow, "execute");
    assert!(execute.contains("needs: [plan, build]"), "{execute}");
    let place = execute
        .find("npx --yes --package @napi-rs/cli@3.10.4 napi artifacts")
        .expect("napi artifacts step");
    let manifest = execute
        .find("callisto release artifact-manifest")
        .expect("artifact-manifest");
    let run = execute
        .find(r#"callisto release execute "${args[@]}""#)
        .expect("release execute");
    assert!(place < manifest && manifest < run, "{execute}");
    assert!(
        execute.contains("CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}"),
        "{execute}"
    );
    assert!(execute.contains("NPM_TOKEN: ${{ secrets.NPM_TOKEN }}"), "{execute}");
    assert!(execute.contains("id-token: write"), "{execute}");
    assert!(
        execute.contains("          NPM_CONFIG_PROVENANCE: \"true\"\n"),
        "{execute}"
    );
    assert!(!workflow.contains("PYPI_TOKEN"), "{workflow}");
    for job_name in ["plan", "build"] {
        let non_publishing = job(&workflow, job_name);
        for secret in ["CARGO_REGISTRY_TOKEN", "NPM_TOKEN", "PYPI_TOKEN"] {
            assert!(!non_publishing.contains(secret), "{job_name}: {non_publishing}");
        }
    }
}

// npm provenance is requested only when the workspace has an npm package.
#[test]
fn matrix_workflow_requests_npm_provenance_only_for_npm_workspaces() {
    let cargo_only = render_workflow(
        &cargo_only_facts(),
        WorkflowShape::BuildMatrix,
        "main",
        FAKE_COMMIT,
        "0.8.0",
    );
    assert!(!cargo_only.contains("NPM_CONFIG_PROVENANCE"), "{cargo_only}");
    assert!(render_matrix_workflow().contains("NPM_CONFIG_PROVENANCE: \"true\""));
}

// The version-pr job matches the simple workflow's.
#[test]
fn matrix_workflow_keeps_the_simple_version_pr_job() {
    let simple = render_workflow_0_8_0(&cargo_only_facts(), "main");
    let matrix = render_matrix_workflow();
    assert_eq!(job(&matrix, "version-pr"), job(&simple, "version-pr"));
    assert!(job(&matrix, "version-pr").contains("with: {mode: version-pr, branch: main}"));
}

// No recovery, coordinator, or SHA-based release-candidate detection; the
// only gate is the decision-file content precondition.
#[test]
fn matrix_workflow_has_no_recovery_or_sha_gating() {
    let workflow = render_matrix_workflow();
    for absent in [
        "recovery",
        "coordinator",
        "release-candidate",
        "release_source_sha",
        "workflow_dispatch",
        "gh api",
        "github.sha",
        "is_release_pr",
    ] {
        assert!(!workflow.contains(absent), "`{absent}` present:\n{workflow}");
    }
    assert!(
            workflow.contains("git diff-tree --no-commit-id --name-only --diff-filter=AM -r HEAD^ HEAD | grep -qxF .callisto/release-decision.json"),
            "{workflow}"
        );
    assert_eq!(
        workflow.matches("steps.decision.outputs.release == 'true'").count(),
        4,
        "{workflow}"
    );
}

/// Real git, except `ls-remote --symref` answers from `remote_head` instead of touching
/// the network -- `workspace()`'s `origin` is a real GitHub URL that must never be contacted.
struct StubRemoteHead {
    remote_head: Option<&'static str>,
}
impl CommandRunner for StubRemoteHead {
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError> {
        RealGitRunner.run(program, args, cwd)
    }
    fn run_quiet(
        &self,
        program: &str,
        args: &[&str],
        cwd: &Path,
        timeout: std::time::Duration,
    ) -> Result<CommandOutput, CommandError> {
        if args.first() == Some(&"ls-remote") {
            return Ok(match self.remote_head {
                Some(branch) => CommandOutput {
                    exit_code: Some(0),
                    stdout: format!("ref: refs/heads/{branch}\tHEAD\n0000000000000000000000000000000000000000\tHEAD\n"),
                    stderr: String::new(),
                },
                None => CommandOutput {
                    exit_code: Some(128),
                    stdout: String::new(),
                    stderr: "fatal: could not read Username for 'https://github.com': terminal prompts disabled\n"
                        .to_owned(),
                },
            });
        }
        RealGitRunner.run_with_timeout(program, args, cwd, timeout)
    }
}

// Local `origin/HEAD` wins over everything else, including a live remote answer.
#[test]
fn default_branch_prefers_local_origin_head() {
    let dir = workspace();
    git(dir.path(), &["branch", "develop"]);
    git(
        dir.path(),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/develop",
        ],
    );
    let resolved = default_branch(
        &StubRemoteHead {
            remote_head: Some("trunk"),
        },
        dir.path(),
    );
    assert_eq!(resolved.name, "develop");
    assert_eq!(resolved.source, DefaultBranchSource::OriginHead);
}

// No local `origin/HEAD`: a live query of `origin` resolves it without a fetch.
#[test]
fn default_branch_queries_the_remote_when_local_origin_head_is_unset() {
    let dir = workspace();
    let resolved = default_branch(
        &StubRemoteHead {
            remote_head: Some("trunk"),
        },
        dir.path(),
    );
    assert_eq!(resolved.name, "trunk");
    assert_eq!(resolved.source, DefaultBranchSource::RemoteHead);
}

// Neither local nor remote resolves a HEAD: falls back to the branch checked out.
#[test]
fn default_branch_falls_back_to_the_current_branch() {
    let dir = workspace();
    git(dir.path(), &["checkout", "-q", "-b", "release"]);
    let resolved = default_branch(&StubRemoteHead { remote_head: None }, dir.path());
    assert_eq!(resolved.name, "release");
    assert_eq!(resolved.source, DefaultBranchSource::CurrentBranch);
}

// No git repository at all: nothing resolves, so `main` is the last resort.
#[test]
fn default_branch_falls_back_to_main_when_nothing_resolves() {
    let none = tempfile::tempdir().unwrap();
    let resolved = default_branch(&RealGitRunner, none.path());
    assert_eq!(resolved.name, "main");
    assert_eq!(resolved.source, DefaultBranchSource::Fallback);
}

// Artifact slots or napi platform packages route to the build matrix; maturin,
// and platform packages with no napi.targets to build them from, are unsupported.
#[test]
fn workflow_shape_routes_slots_and_napi_to_the_build_matrix() {
    let facts = cargo_only_facts();
    assert_eq!(workflow_shape(&facts, &INDEPENDENT), Ok(WorkflowShape::Simple));
    assert_eq!(
        workflow_shape(&facts, &release(&["x86_64-unknown-linux-gnu"])),
        Ok(WorkflowShape::BuildMatrix)
    );
    let napi = facts_for_render();
    assert_eq!(workflow_shape(&napi, &INDEPENDENT), Ok(WorkflowShape::BuildMatrix));

    let no_targets = InitFacts {
        has_napi_targets: false,
        ..facts_for_render()
    };
    assert_eq!(
        workflow_shape(&no_targets, &INDEPENDENT),
        Err(WorkflowUnsupported::PlatformPackagesWithoutNapiTargets)
    );
    let maturin = InitFacts {
        has_maturin_targets: true,
        ..facts_for_render()
    };
    assert_eq!(
        workflow_shape(&maturin, &release(&["x86_64-unknown-linux-gnu"])),
        Err(WorkflowUnsupported::Maturin)
    );
    assert!(WorkflowUnsupported::Maturin.to_string().contains("maturin"));
}

// detect reads napi and maturin targets from `callisto matrix`.
#[test]
fn detect_reports_napi_and_maturin_targets() {
    let napi = repo(
        &[(
            "web/package.json",
            r#"{"name":"web","version":"1.0.0","napi":{"targets":["x86_64-unknown-linux-gnu"]}}"#,
        )],
        Some(ORIGIN),
    );
    let facts = detect_in(napi.path()).unwrap();
    assert!(facts.has_napi_targets && !facts.has_maturin_targets);

    let maturin = repo(
            &[(
                "py/pyproject.toml",
                "[project]\nname = \"py\"\nversion = \"1.0.0\"\n\n[tool.maturin]\ntargets = [\"x86_64-unknown-linux-gnu\"]\n",
            )],
            Some(ORIGIN),
        );
    let facts = detect_in(maturin.path()).unwrap();
    assert!(facts.has_maturin_targets && !facts.has_napi_targets);
}

// A platform package attached from a sibling directory (napi's per-arch
// packages) never becomes its own entry in `packages`, so
// `has_platform_packages` -- not a per-package scan -- must catch it.
#[test]
fn detect_flags_a_workspace_with_only_an_attached_platform_package() {
    let dir = repo(
        &[
            (
                "web/package.json",
                r#"{"name":"web","version":"1.0.0","optionalDependencies":{"web-linux-x64-gnu":"1.0.0"}}"#,
            ),
            (
                "web-linux-x64-gnu/package.json",
                r#"{"name":"web-linux-x64-gnu","version":"1.0.0","os":["linux"],"cpu":["x64"]}"#,
            ),
        ],
        Some(ORIGIN),
    );
    let facts = detect_in(dir.path()).unwrap();
    assert!(
        facts.packages.iter().all(|package| !package.platform),
        "an attached platform package must not become its own InitPackage: {:?}",
        facts.packages
    );
    assert!(
        facts.has_platform_packages,
        "the attached platform package must still be detected"
    );
    assert_eq!(
        workflow_shape(&facts, &INDEPENDENT),
        Err(WorkflowUnsupported::PlatformPackagesWithoutNapiTargets)
    );
}

/// Answers `git ls-remote` with canned output; no network involved.
struct FakeLsRemote {
    stdout: &'static str,
    exit_code: Option<i32>,
}
impl CommandRunner for FakeLsRemote {
    fn run(&self, _: &str, _: &[&str], _: &Path) -> Result<CommandOutput, CommandError> {
        Ok(CommandOutput {
            exit_code: self.exit_code,
            stdout: self.stdout.to_owned(),
            stderr: String::new(),
        })
    }
}

// An annotated tag resolves to its peeled (dereferenced) commit,
// not the tag object's own SHA -- `uses:` needs a real commit.
#[test]
fn resolve_release_commit_prefers_the_peeled_commit_over_the_tag_object() {
    let runner = FakeLsRemote {
        stdout: "6162626263636464646262626363646464626262\trefs/tags/callisto@0.8.0\n\
                     6363646464626262636364646262626363646464\trefs/tags/callisto@0.8.0^{}\n",
        exit_code: Some(0),
    };
    let commit = resolve_release_commit(&runner, Path::new("."), "0.8.0").unwrap();
    assert_eq!(commit, "6363646464626262636364646262626363646464");
}

// A lightweight tag has no peeled line; its own SHA already is the commit.
#[test]
fn resolve_release_commit_falls_back_to_the_tag_object_for_a_lightweight_tag() {
    let runner = FakeLsRemote {
        stdout: "6161616161626262626363636364646464626262\trefs/tags/callisto@0.8.0\n",
        exit_code: Some(0),
    };
    let commit = resolve_release_commit(&runner, Path::new("."), "0.8.0").unwrap();
    assert_eq!(commit, "6161616161626262626363636364646464626262");
}

// An unresolvable tag (offline, or an unreleased version) errors
// clearly instead of embedding a broken reference in the generated file.
#[test]
fn resolve_release_commit_errors_when_the_tag_is_not_found() {
    let empty = FakeLsRemote {
        stdout: "",
        exit_code: Some(0),
    };
    let error = resolve_release_commit(&empty, Path::new("."), "9.9.9").unwrap_err();
    assert!(
        matches!(&error, GraphError::InitWorkflowVersionUnresolved { version } if version == "9.9.9"),
        "{error}"
    );

    let failed = FakeLsRemote {
        stdout: "",
        exit_code: Some(128),
    };
    assert!(matches!(
        resolve_release_commit(&failed, Path::new("."), "0.8.0").unwrap_err(),
        GraphError::InitWorkflowVersionUnresolved { .. }
    ));
}

struct NoGit;
impl CommandRunner for NoGit {
    fn run(&self, program: &str, _: &[&str], _: &Path) -> Result<CommandOutput, CommandError> {
        Err(CommandError::NotFound {
            program: program.to_owned(),
        })
    }
}

// A transport-level failure (no git, network unreachable) keeps its real
// cause via GraphError::Command, rather than being discarded.
#[test]
fn resolve_release_commit_preserves_a_transport_failures_real_cause() {
    let error = resolve_release_commit(&NoGit, Path::new("."), "0.8.0").unwrap_err();
    assert!(
        matches!(&error, GraphError::Command(CommandError::NotFound { .. })),
        "{error}"
    );
}

// Underlies the workflow file's own no-overwrite check and write.
#[test]
fn write_workflow_refuses_an_existing_file_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let permit = ApplyPermit::force_for_tests();
    assert_eq!(
        workflow_path(dir.path()),
        dir.path().join(".github/workflows/callisto-release.yml")
    );
    ensure_workflow_absent(dir.path()).unwrap();
    write_workflow(dir.path(), "content\n", &permit).unwrap();
    assert_eq!(std::fs::read_to_string(workflow_path(dir.path())).unwrap(), "content\n");

    let error = ensure_workflow_absent(dir.path()).unwrap_err();
    assert!(matches!(&error, GraphError::InitWorkflowExists { path } if *path == workflow_path(dir.path())));
    let error = write_workflow(dir.path(), "other\n", &permit).unwrap_err();
    assert!(matches!(&error, GraphError::InitWorkflowExists { .. }), "{error}");
    assert_eq!(std::fs::read_to_string(workflow_path(dir.path())).unwrap(), "content\n");
}

// Every unhonoured `@changesets/cli` key present warns once; `fixed` names its
// callisto.toml replacement, the rest have none to name.
#[test]
fn changesets_config_diagnostics_warns_per_unhonoured_key_present() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".changeset")).unwrap();
    std::fs::write(
        dir.path().join(".changeset/config.json"),
        r#"{
                "fixed": [["pkg-a", "pkg-b"]],
                "linked": [["pkg-c", "pkg-d"]],
                "access": "public",
                "changelog": false
            }"#,
    )
    .unwrap();

    let diagnostics = changesets_config_diagnostics(dir.path());
    let codes: Vec<_> = diagnostics.iter().map(|d| d.code).collect();
    assert_eq!(
        codes,
        vec![DiagnosticCode::ChangesetsConfigKeyDropped; 3],
        "only fixed, linked and access are present among the known keys; changelog is out of scope"
    );
    assert!(diagnostics.iter().all(|d| d.severity == DiagnosticSeverity::Warning));
    assert!(diagnostics
        .iter()
        .all(|d| d.path.as_deref() == Some(dir.path().join(".changeset/config.json").as_path())));

    let fixed = diagnostics.iter().find(|d| d.message.contains("`fixed`")).unwrap();
    assert!(
        fixed.message.contains("[[fixed-group]]"),
        "fixed must name its callisto.toml equivalent: {}",
        fixed.message
    );
    let linked = diagnostics.iter().find(|d| d.message.contains("`linked`")).unwrap();
    assert!(
        linked.message.contains("no equivalent"),
        "linked has no callisto.toml equivalent: {}",
        linked.message
    );
}

// No config.json, or one that isn't a JSON object, is silently ignored -- not every
// workspace has opted into `@changesets/cli` compatibility.
#[test]
fn changesets_config_diagnostics_is_empty_without_a_config_object() {
    let dir = tempfile::tempdir().unwrap();
    assert!(changesets_config_diagnostics(dir.path()).is_empty(), "no file at all");

    std::fs::create_dir_all(dir.path().join(".changeset")).unwrap();
    std::fs::write(dir.path().join(".changeset/config.json"), "[1, 2, 3]").unwrap();
    assert!(
        changesets_config_diagnostics(dir.path()).is_empty(),
        "not a JSON object"
    );

    std::fs::write(dir.path().join(".changeset/config.json"), "{\"commit\": true}").unwrap();
    assert!(
        changesets_config_diagnostics(dir.path()).is_empty(),
        "no unhonoured key present"
    );
}
