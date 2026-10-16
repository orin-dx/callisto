//! A Cargo dependency key declared in several tables is several declarations: each is its own
//! edge and is rewritten on its own, whatever the other tables say.

use std::fs;
use std::path::Path;

use callisto_fixtures::git::GitRunner;
use callisto_graph::commands::{plan_snapshot, plan_version, VersionOptions};
use callisto_graph::infer::NoInference;
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::{apply_version_plan, ApplyOptions, DependencyResolver, Workspace};
use callisto_model::{ApplyPermit, DepKind, DepLocator, PackageId};

const B_MANIFEST: &str = "[package]\nname = \"b\"\nversion = \"1.0.0\"\nedition = \"2021\"\n";

fn manifest(dependencies: &str) -> String {
    format!("[package]\nname = \"a\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n{dependencies}")
}

/// A repository holding `files`, with one commit.
fn repository(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    for (path, content) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    for args in [["add", "."], ["commit", "-q"]] {
        let mut command = std::process::Command::new("git");
        command.args(args).current_dir(root);
        if args[0] == "commit" {
            command.args(["-m", "init"]);
        }
        command.output().expect("git must be installed");
    }
    dir
}

fn load(root: &Path) -> Workspace<'static, GitRunner> {
    let locator = IgnoreWalkLocator::new(root);
    Workspace::load(root.to_path_buf(), &locator, &callisto_fixtures::git::GitRunner).unwrap()
}

/// Releases `changeset` and returns the text of every file in `read`, in order.
fn release(files: &[(&str, &str)], changeset: &str, read: &[&str]) -> Vec<String> {
    let dir = repository(files);
    let root = dir.path();
    fs::create_dir_all(root.join(".changeset")).unwrap();
    fs::write(root.join(".changeset/change.md"), changeset).unwrap();
    let ws = load(root);
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };
    let plan = plan_version(&ws, &NoInference, &opts).unwrap();
    apply_version_plan(
        root,
        &plan,
        &callisto_fixtures::git::GitRunner,
        &ApplyOptions {
            refresh_lockfiles: false,
            transient: true,
        },
        &ApplyPermit::force_for_tests(),
    )
    .unwrap();
    read.iter()
        .map(|path| fs::read_to_string(root.join(path)).unwrap())
        .collect()
}

const MINOR_B: &str = "---\n\"b\": minor\n---\n\nChange.\n";

fn released_a(dependencies: &str) -> String {
    let a = manifest(dependencies);
    release(
        &[("a/Cargo.toml", &a), ("b/Cargo.toml", B_MANIFEST)],
        MINOR_B,
        &["a/Cargo.toml"],
    )
    .remove(0)
}

fn released_version(dependencies: &str) -> String {
    manifest(dependencies).replace("version = \"1.0.0\"", "version = \"1.0.1\"")
}

#[test]
fn each_table_declaring_a_key_is_its_own_edge() {
    let a = manifest(
        "[dependencies]\nb = \"^1.0.0\"\n\n[dev-dependencies]\nb = \"1.0.0\"\n\n[target.'cfg(unix)'.dependencies]\nb = \"=1.0.0\"\n\n[target.'cfg(windows)'.build-dependencies]\nb = \"1.0.0\"\n",
    );
    let dir = repository(&[("a/Cargo.toml", &a), ("b/Cargo.toml", B_MANIFEST)]);
    let ws = load(dir.path());
    let a_id = PackageId::parse("a").unwrap();
    let mut edges: Vec<_> = ws
        .graph
        .dependencies_of(&a_id)
        .map(|edge| (edge.kind, edge.locator.clone(), edge.spec.render()))
        .collect();
    edges.sort();
    let locator = |section, target: Option<&str>| DepLocator::cargo(section, target.map(str::to_string));
    assert_eq!(
        edges,
        [
            (DepKind::Runtime, locator(DepKind::Runtime, None), "^1.0.0".to_string()),
            (
                DepKind::Runtime,
                locator(DepKind::Runtime, Some("cfg(unix)")),
                "=1.0.0".to_string()
            ),
            (DepKind::Dev, locator(DepKind::Dev, None), "1.0.0".to_string()),
            (
                DepKind::Build,
                locator(DepKind::Build, Some("cfg(windows)")),
                "1.0.0".to_string()
            ),
        ]
    );
}

#[test]
fn a_stale_target_declaration_follows_the_bump_beside_a_plain_one() {
    let dependencies = "\
[dependencies]
b = \"^1.0.0\"

# unix only
[target.'cfg(unix)'.dependencies]
b = \"=1.0.0\" # exact

[target.'cfg(target_os = \"linux\")'.dependencies]
b = { version = \">=1.0.0, <2.0.0\", features = [\"x\"] }

[target.x86_64-pc-windows-gnu.dependencies.b]
version = \"1.0.0\" # dotted table
";
    let expected = "\
[dependencies]
b = \"^1.1.0\"

# unix only
[target.'cfg(unix)'.dependencies]
b = \"=1.1.0\" # exact

[target.'cfg(target_os = \"linux\")'.dependencies]
b = { version = \">=1.1.0, <2.0.0\", features = [\"x\"] }

[target.x86_64-pc-windows-gnu.dependencies.b]
version = \"1.1.0\" # dotted table
";
    assert_eq!(released_a(dependencies), released_version(expected));
}

#[test]
fn only_the_stale_declaration_changes_when_the_other_is_already_current() {
    let dependencies = "\
[dependencies]
b = \"=1.1.0\" # keep

[target.'cfg(unix)'.dependencies]
b = \"=1.0.0\"
";
    let expected = dependencies.replacen("\"=1.0.0\"", "\"=1.1.0\"", 1);
    assert_eq!(released_a(dependencies), released_version(&expected));
}

#[test]
fn a_renamed_dependency_declared_twice_is_rewritten_in_both_places() {
    let dependencies = "\
[dependencies]
bee = { package = \"b\", version = \"^1.0.0\" }

[target.'cfg(unix)'.dependencies]
bee = { version = \"=1.0.0\", package = \"b\" }
";
    let expected = "\
[dependencies]
bee = { package = \"b\", version = \"^1.1.0\" }

[target.'cfg(unix)'.dependencies]
bee = { version = \"=1.1.0\", package = \"b\" }
";
    assert_eq!(released_a(dependencies), released_version(expected));
}

#[test]
fn one_key_naming_different_crates_in_two_tables_rewrites_only_the_bumped_crate() {
    let dependencies = "\
[dependencies]
dep = { package = \"b\", version = \"=1.0.0\" }

[target.'cfg(unix)'.dependencies]
dep = { package = \"c\", version = \"=1.0.0\" }
";
    let expected = dependencies.replacen("\"=1.0.0\"", "\"=1.1.0\"", 1);
    let a = manifest(dependencies);
    let c = B_MANIFEST.replace("name = \"b\"", "name = \"c\"");
    let files = [
        ("a/Cargo.toml", a.as_str()),
        ("b/Cargo.toml", B_MANIFEST),
        ("c/Cargo.toml", c.as_str()),
    ];
    assert_eq!(
        release(&files, MINOR_B, &["a/Cargo.toml"]),
        [released_version(&expected)]
    );
}

#[test]
fn dev_and_build_declarations_are_rewritten_in_every_table() {
    let dependencies = "\
[dev-dependencies]
b = \"=1.0.0\"

[build-dependencies]
b = \"=1.0.0\"

[target.'cfg(unix)'.dev-dependencies]
b = \"=1.0.0\"

[target.'cfg(unix)'.build-dependencies]
b = { version = \"=1.0.0\", optional = true }
";
    let expected = dependencies.replace("\"=1.0.0\"", "\"=1.1.0\"");
    assert_eq!(released_a(dependencies), released_version(&expected));
}

#[test]
fn an_inherited_declaration_and_a_target_declaration_are_rewritten_separately() {
    let root_manifest = "[workspace]\nmembers = [\"a\", \"b\"]\n\n[workspace.dependencies]\nb = { version = \"=1.0.0\", path = \"b\" }\n";
    let dependencies = "\
[dependencies]
b = { workspace = true }

[target.'cfg(unix)'.dependencies]
b = \"=1.0.0\"
";
    let expected = "\
[dependencies]
b = { workspace = true }

[target.'cfg(unix)'.dependencies]
b = \"=1.1.0\"
";
    let a = manifest(dependencies);
    let files = [
        ("Cargo.toml", root_manifest),
        ("a/Cargo.toml", a.as_str()),
        ("b/Cargo.toml", B_MANIFEST),
    ];
    assert_eq!(
        release(&files, MINOR_B, &["a/Cargo.toml", "Cargo.toml"]),
        [
            released_version(expected),
            root_manifest.replace("\"=1.0.0\"", "\"=1.1.0\"")
        ]
    );
}

#[test]
fn an_inherited_declaration_in_two_tables_writes_the_workspace_dependency_once() {
    let root_manifest = "[workspace]\nmembers = [\"a\", \"b\"]\n\n[workspace.dependencies]\nb = { version = \"=1.0.0\", path = \"b\" }\n";
    let dependencies = "\
[dependencies]
b = { workspace = true }

[target.'cfg(unix)'.dependencies]
b = { workspace = true }
";
    let a = manifest(dependencies);
    let files = [
        ("Cargo.toml", root_manifest),
        ("a/Cargo.toml", a.as_str()),
        ("b/Cargo.toml", B_MANIFEST),
    ];
    assert_eq!(
        release(&files, MINOR_B, &["a/Cargo.toml", "Cargo.toml"]),
        [
            released_version(dependencies),
            root_manifest.replace("\"=1.0.0\"", "\"=1.1.0\"")
        ]
    );

    // Two member tables name one workspace dependency, so the plan holds one rewrite of it, not two.
    let dir = repository(&files);
    fs::create_dir_all(dir.path().join(".changeset")).unwrap();
    fs::write(dir.path().join(".changeset/change.md"), MINOR_B).unwrap();
    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };
    let plan = plan_version(&load(dir.path()), &NoInference, &opts).unwrap();
    let rewrites = plan.rewrites.iter().filter(|rewrite| rewrite.key.name == "b").count();
    assert_eq!(rewrites, 1, "{:?}", plan.rewrites);
}

#[test]
fn a_target_table_that_names_no_such_dependency_is_an_error_not_a_silent_skip() {
    use callisto_manifests::{CargoToml, Manifest, OpenContext};
    use callisto_model::{DepSpec, ManifestDecl, ManifestError, ManifestFormat, ManifestRole, Version};

    let dir = repository(&[("Cargo.toml", &manifest("[dependencies]\nb = \"=1.0.0\"\n"))]);
    let decl = ManifestDecl::new("Cargo.toml", ManifestRole::Canonical, ManifestFormat::CargoToml).unwrap();
    let ctx = OpenContext {
        workspace_root: dir.path(),
        cargo_workspace: None,
        npm_workspace_kind: None,
    };
    let mut cargo = CargoToml::open(&decl, &ctx).unwrap();
    let at = DepLocator::cargo(DepKind::Runtime, Some("cfg(unix)".to_string()));
    let result = cargo.update_dependency_spec(
        "b",
        DepKind::Runtime,
        &at,
        DepSpec::CargoBare(Version::semver(1, 1, 0)),
        &ApplyPermit::force_for_tests(),
    );
    assert!(
        matches!(result, Err(ManifestError::DependencyNotFound { .. })),
        "{result:?}"
    );
}

#[test]
fn a_snapshot_rewrites_every_declaration_of_a_key() {
    let a = manifest("[dependencies]\nb = \"^1.0.0\"\n\n[target.'cfg(unix)'.dependencies]\nb = \"=1.0.0\"\n");
    let dir = repository(&[("a/Cargo.toml", &a), ("b/Cargo.toml", B_MANIFEST)]);
    let root = dir.path();
    let ws = load(root);
    let (plan, report) = plan_snapshot(&ws, "preview").unwrap();
    let tag = &report.snapshot_tag;
    apply_version_plan(
        root,
        &plan,
        &callisto_fixtures::git::GitRunner,
        &ApplyOptions {
            refresh_lockfiles: false,
            transient: true,
        },
        &ApplyPermit::force_for_tests(),
    )
    .unwrap();
    let after = fs::read_to_string(root.join("a/Cargo.toml")).unwrap();
    let expected = format!("[dependencies]\nb = \"^{tag}\"\n\n[target.'cfg(unix)'.dependencies]\nb = \"={tag}\"\n");
    assert!(after.ends_with(&expected), "{after}");
}
