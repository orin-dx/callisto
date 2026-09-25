//! npm platform packages (`os`+`cpu`) named in an owner's
//! `optionalDependencies` are `Platform` manifests of that owner, never
//! `Package`s of their own. Fixture mirrors oxc-react-docgen: a napi addon and
//! an esbuild-style CLI, each with standalone `npm/<platform>` directories
//! outside the pnpm workspace globs.

use std::path::{Path, PathBuf};

use callisto_graph::commands::{plan_version, VersionOptions};
use callisto_graph::locate::IgnoreWalkLocator;
use callisto_graph::{DependencyResolver, NoInference, Workspace};
use callisto_model::{DiagnosticCode, ManifestRole, PackageId};

use callisto_fixtures::git::GitRunner;

fn git(root: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git must be installed");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

const PLATFORMS: [(&str, &str, &str, Option<&str>); 2] = [
    ("darwin-arm64", "darwin", "arm64", None),
    ("linux-x64-gnu", "linux", "x64", Some("glibc")),
];

fn owner_json(name: &str, version: &str, prefix: &str, napi: bool) -> String {
    let deps = PLATFORMS
        .iter()
        .map(|(suffix, ..)| format!(r#""{prefix}-{suffix}":"{version}""#))
        .collect::<Vec<_>>()
        .join(",");
    let napi = if napi {
        r#","napi":{"binaryName":"addon","targets":["aarch64-apple-darwin","x86_64-unknown-linux-gnu"]}"#
    } else {
        ""
    };
    format!(r#"{{"name":"{name}","version":"{version}","optionalDependencies":{{{deps}}}{napi}}}"#)
}

fn platform_json(name: &str, version: &str, os: &str, cpu: &str, libc: Option<&str>) -> String {
    let libc = libc.map(|l| format!(r#","libc":["{l}"]"#)).unwrap_or_default();
    format!(r#"{{"name":"{name}","version":"{version}","os":["{os}"],"cpu":["{cpu}"]{libc},"main":"index.js"}}"#)
}

/// Returns the workspace root of a fresh oxc-react-docgen-shaped fixture.
pub fn build_fixture(root: &Path) {
    callisto_fixtures::git::init_repo(root);
    write(
        root,
        "package.json",
        r#"{"name":"workspace-root","version":"0.0.0","private":true}"#,
    );
    write(root, "pnpm-workspace.yaml", "packages:\n  - \"packages/*\"\n");
    write(root, "pnpm-lock.yaml", "lockfileVersion: '9.0'\n");
    for (dir, scope, napi) in [("napi", "@s/napi", true), ("cli", "@s/cli", false)] {
        write(
            root,
            &format!("packages/{dir}/package.json"),
            &owner_json(scope, "0.1.0", scope, napi),
        );
        for (suffix, os, cpu, libc) in PLATFORMS {
            write(
                root,
                &format!("packages/{dir}/npm/{suffix}/package.json"),
                &platform_json(&format!("{scope}-{suffix}"), "0.1.0", os, cpu, libc),
            );
        }
    }
    write(root, "callisto.toml", "");
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "init"]);
}

fn load(root: &Path) -> Workspace<'static, GitRunner> {
    static RUNNER: GitRunner = GitRunner;
    Workspace::load(root.to_path_buf(), &IgnoreWalkLocator::new(root), &RUNNER).expect("workspace must load")
}

fn platform_paths(ws: &Workspace<'_, GitRunner>, owner: &str) -> Vec<PathBuf> {
    let pkg = ws
        .graph
        .packages()
        .find(|p| p.id.name() == owner)
        .unwrap_or_else(|| panic!("owner `{owner}` must be a package"));
    pkg.manifests
        .iter()
        .filter(|m| matches!(m.role, ManifestRole::Platform { .. }))
        .map(|m| m.path.clone())
        .collect()
}

#[test]
fn platform_packages_are_manifests_of_their_owner_not_packages() {
    let tmp = tempfile::tempdir().unwrap();
    build_fixture(tmp.path());
    let ws = load(tmp.path());

    let mut names: Vec<_> = ws.graph.packages().map(|p| p.id.name().to_string()).collect();
    names.sort();
    assert_eq!(names, ["@s/cli", "@s/napi", "workspace-root"]);

    for dir in ["napi", "cli"] {
        let owner = format!("@s/{dir}");
        assert_eq!(
            platform_paths(&ws, &owner),
            PLATFORMS
                .iter()
                .map(|(suffix, ..)| PathBuf::from(format!("packages/{dir}/npm/{suffix}/package.json")))
                .collect::<Vec<_>>(),
        );
        for (suffix, ..) in PLATFORMS {
            let (plat_owner, _, _) = ws
                .identity
                .platform
                .get(&format!("{owner}-{suffix}"))
                .expect("platform must be indexed by its own name");
            assert_eq!(plat_owner, &PackageId::Bare(owner.clone()));
        }
    }
    assert!(
        !ws.graph
            .diagnostics()
            .iter()
            .any(|d| d.code == DiagnosticCode::PlatformPackageWithoutOwner),
        "every platform has an owner: {:?}",
        ws.graph.diagnostics()
    );
}

#[test]
fn platform_package_without_owner_stays_a_package_with_a_diagnostic() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    write(
        root,
        "packages/stray/package.json",
        &platform_json("@s/stray-linux-x64-gnu", "0.1.0", "linux", "x64", None),
    );
    let ws = load(root);

    assert!(ws.graph.packages().any(|p| p.id.name() == "@s/stray-linux-x64-gnu"));
    let diag = ws
        .graph
        .diagnostics()
        .iter()
        .find(|d| d.code == DiagnosticCode::PlatformPackageWithoutOwner)
        .expect("an ownerless platform package must be diagnosed");
    assert_eq!(diag.path.as_deref(), Some(Path::new("packages/stray/package.json")));
}

#[test]
fn platform_package_named_by_two_owners_is_not_guessed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    write(
        root,
        "packages/other/package.json",
        r#"{"name":"@s/other","version":"0.1.0","optionalDependencies":{"@s/cli-darwin-arm64":"0.1.0"}}"#,
    );
    let ws = load(root);

    assert!(!ws.identity.platform.contains_key("@s/cli-darwin-arm64"));
    assert!(!platform_paths(&ws, "@s/cli").contains(&PathBuf::from("packages/cli/npm/darwin-arm64/package.json")));
    let diag = ws
        .graph
        .diagnostics()
        .iter()
        .find(|d| d.code == DiagnosticCode::PlatformPackageWithoutOwner)
        .expect("an ambiguous owner must be diagnosed");
    assert!(diag.message.contains("2 packages"), "{}", diag.message);
}

#[test]
fn workspace_member_platform_packages_attach_too() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    write(
        root,
        "pnpm-workspace.yaml",
        "packages:\n  - \"packages/*\"\n  - \"packages/*/npm/*\"\n",
    );
    let ws = load(root);

    let mut names: Vec<_> = ws.graph.packages().map(|p| p.id.name().to_string()).collect();
    names.sort();
    assert_eq!(names, ["@s/cli", "@s/napi", "workspace-root"]);
    assert_eq!(platform_paths(&ws, "@s/cli").len(), PLATFORMS.len());
}

#[test]
fn unnamed_platform_outside_the_workspace_stays_undiscovered() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    // napi-rs `create-npm-dirs` output before `prepublish` adds optionalDependencies.
    write(
        root,
        "packages/napi/npm/win32-x64-msvc/package.json",
        &platform_json("@s/napi-win32-x64-msvc", "0.1.0", "win32", "x64", None),
    );
    let ws = load(root);

    assert!(!ws.graph.packages().any(|p| p.id.name() == "@s/napi-win32-x64-msvc"));
    assert!(!ws.identity.platform.contains_key("@s/napi-win32-x64-msvc"));
    assert!(ws.graph.diagnostics().is_empty(), "{:?}", ws.graph.diagnostics());
}

fn changeset(root: &Path, body: &str) {
    write(root, ".changeset/bump.md", body);
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "changeset"]);
}

#[test]
fn platform_versions_follow_the_owner_without_a_fixed_group() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_fixture(root);
    changeset(root, "---\n\"@s/cli\": minor\n---\n\nfeat.\n");
    let ws = load(root);

    let opts = VersionOptions {
        strict: false,
        allow_empty_changesets: true,
    };
    let plan = plan_version(&ws, &NoInference, &opts).expect("plan_version must succeed");

    let target = "0.2.0".to_string();
    let bumped: Vec<_> = plan.bumps.iter().map(|b| b.package.name().to_string()).collect();
    assert_eq!(bumped, ["@s/cli"], "platform packages are not bumped as packages");

    let mut writes: Vec<_> = plan
        .platform_writes
        .iter()
        .map(|w| (w.manifest.clone(), w.version.render().to_string()))
        .collect();
    writes.sort();
    assert_eq!(
        writes,
        PLATFORMS
            .iter()
            .map(|(suffix, ..)| (
                PathBuf::from(format!("packages/cli/npm/{suffix}/package.json")),
                target.clone()
            ))
            .collect::<Vec<_>>(),
    );

    assert_eq!(plan.optional_dep_updates.len(), 1);
    let update = &plan.optional_dep_updates[0];
    assert_eq!(update.manifest, Path::new("packages/cli/package.json"));
    let mut pins: Vec<_> = update
        .updates
        .iter()
        .map(|(n, v)| (n.clone(), v.render().to_string()))
        .collect();
    pins.sort();
    assert_eq!(
        pins,
        PLATFORMS
            .iter()
            .map(|(suffix, ..)| (format!("@s/cli-{suffix}"), target.clone()))
            .collect::<Vec<_>>(),
    );
}

#[test]
fn snapshot_versions_attached_platforms_too() {
    let tmp = tempfile::tempdir().unwrap();
    build_fixture(tmp.path());
    let ws = load(tmp.path());
    let (plan, _) = callisto_graph::commands::plan_snapshot(&ws, "canary").expect("plan_snapshot must succeed");
    let snapshot = &plan.bumps.iter().find(|b| b.package.name() == "@s/napi").unwrap().to;
    let napi_writes: Vec<_> = plan
        .platform_writes
        .iter()
        .filter(|w| w.manifest.starts_with("packages/napi"))
        .collect();
    assert_eq!(napi_writes.len(), PLATFORMS.len());
    assert!(napi_writes.iter().all(|w| &w.version == snapshot));
}
