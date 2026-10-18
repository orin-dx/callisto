//! Which Cargo workspace governs a package, checked against `cargo locate-project --workspace`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use callisto_manifests::{
    cargo_workspace_of, enclosing_workspace_manifest, parse_manifest, workspace_manifest_candidates,
};

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn package(name: &str, extra: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{extra}")
}

/// The workspace manifest Cargo itself reports for the package at `manifest`, relative to `root`.
fn cargo_says(root: &Path, manifest: &str) -> Option<PathBuf> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .args([
            "locate-project",
            "--workspace",
            "--message-format",
            "plain",
            "--manifest-path",
        ])
        .arg(root.join(manifest))
        .output()
        .expect("cargo runs");
    if !output.status.success() {
        return None;
    }
    let reported = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    let root = fs::canonicalize(root).unwrap();
    Some(
        fs::canonicalize(reported)
            .unwrap()
            .strip_prefix(root)
            .unwrap()
            .to_path_buf(),
    )
}

fn callisto_says(root: &Path, manifest: &str) -> Option<PathBuf> {
    let text = fs::read_to_string(root.join(manifest)).unwrap();
    let document = parse_manifest(Path::new(manifest), &text).unwrap();
    enclosing_workspace_manifest(root, Path::new(manifest), &document).unwrap()
}

/// A label, the files of a layout, the member asked about and the workspace manifest it belongs to.
type Layout<'a> = (&'a str, &'a [(&'a str, String)], &'a str, &'a str);

/// Each layout is one Cargo accepts; Callisto must name the same workspace manifest.
#[test]
fn the_governing_workspace_is_the_one_cargo_reports() {
    let layouts: &[Layout] = &[
        (
            "nearest ancestor with a workspace table",
            &[
                ("Cargo.toml", "[workspace]\nmembers = [\"a\"]\n".into()),
                ("a/Cargo.toml", package("a", "")),
            ],
            "a/Cargo.toml",
            "Cargo.toml",
        ),
        (
            "a workspace nested under another is its own",
            &[
                ("Cargo.toml", "[workspace]\nmembers = [\"a\"]\n".into()),
                ("a/Cargo.toml", package("a", "")),
                ("native/Cargo.toml", "[workspace]\nmembers = [\"n\"]\n".into()),
                ("native/n/Cargo.toml", package("n", "")),
            ],
            "native/n/Cargo.toml",
            "native/Cargo.toml",
        ),
        (
            "a workspace below the repository root with none above it",
            &[
                ("native/Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n".into()),
                ("native/crates/a/Cargo.toml", package("a", "")),
            ],
            "native/crates/a/Cargo.toml",
            "native/Cargo.toml",
        ),
        (
            "a package that is also the workspace root",
            &[("Cargo.toml", format!("{}[workspace]\n", package("r", "")))],
            "Cargo.toml",
            "Cargo.toml",
        ),
        (
            "an inline workspace table",
            &[
                ("Cargo.toml", "workspace = { members = [\"a\"] }\n".into()),
                ("a/Cargo.toml", package("a", "")),
            ],
            "a/Cargo.toml",
            "Cargo.toml",
        ),
        (
            "a member reached only through a path dependency of the root package",
            &[
                (
                    "Cargo.toml",
                    format!(
                        "{}[dependencies]\nb = {{ path = \"b\" }}\n[workspace]\n",
                        package("r", "")
                    ),
                ),
                ("b/Cargo.toml", package("b", "")),
            ],
            "b/Cargo.toml",
            "Cargo.toml",
        ),
        (
            "package.workspace naming an ancestor directory",
            &[
                ("Cargo.toml", "[workspace]\nmembers = [\"other/z\"]\n".into()),
                ("other/z/Cargo.toml", package("z", "workspace = \"../..\"\n")),
            ],
            "other/z/Cargo.toml",
            "Cargo.toml",
        ),
        (
            "package.workspace naming a sibling directory",
            &[
                ("w/Cargo.toml", "[workspace]\nmembers = [\"../p\"]\n".into()),
                ("p/Cargo.toml", package("p", "workspace = \"../w\"\n")),
            ],
            "p/Cargo.toml",
            "w/Cargo.toml",
        ),
    ];
    for (label, files, member, expected) in layouts {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in *files {
            write(dir.path(), path, text);
            if path.ends_with("Cargo.toml") && !text.contains("[package]") {
                continue;
            }
            let lib = Path::new(path).with_file_name("src/lib.rs");
            write(dir.path(), &lib.to_string_lossy(), "");
        }
        assert_eq!(
            cargo_says(dir.path(), member).as_deref(),
            Some(Path::new(expected)),
            "cargo: {label}"
        );
        assert_eq!(
            callisto_says(dir.path(), member).as_deref(),
            Some(Path::new(expected)),
            "callisto: {label}"
        );
    }
}

#[test]
fn a_workspace_table_that_exists_only_as_workspace_package_still_governs() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "Cargo.toml", "[workspace.package]\nversion = \"1.0.0\"\n");
    write(dir.path(), "a/Cargo.toml", &package("a", ""));
    assert_eq!(
        callisto_says(dir.path(), "a/Cargo.toml").as_deref(),
        Some(Path::new("Cargo.toml"))
    );
}

#[test]
fn no_workspace_above_the_package_means_none() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "Cargo.toml", &package("r", ""));
    write(dir.path(), "a/Cargo.toml", &package("a", ""));
    assert_eq!(callisto_says(dir.path(), "a/Cargo.toml"), None);
    assert_eq!(
        cargo_workspace_of(dir.path(), Path::new("a/Cargo.toml")).unwrap(),
        Path::new("a/Cargo.toml")
    );
}

#[test]
fn the_search_never_leaves_the_workspace_root() {
    let outer = tempfile::tempdir().unwrap();
    write(outer.path(), "Cargo.toml", "[workspace]\nmembers = [\"repo/a\"]\n");
    write(outer.path(), "repo/a/Cargo.toml", &package("a", ""));
    let root = outer.path().join("repo");
    let text = fs::read_to_string(root.join("a/Cargo.toml")).unwrap();
    let document = parse_manifest(Path::new("a/Cargo.toml"), &text).unwrap();
    assert_eq!(
        enclosing_workspace_manifest(&root, Path::new("a/Cargo.toml"), &document).unwrap(),
        None
    );
}

#[test]
fn an_unreadable_manifest_on_the_way_up_is_an_error_not_a_reason_to_keep_climbing() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "Cargo.toml", "[workspace]\nmembers = [\"mid/a\"]\n");
    write(dir.path(), "mid/Cargo.toml", "[workspace\nmembers = [\n");
    write(dir.path(), "mid/a/Cargo.toml", &package("a", ""));
    let text = fs::read_to_string(dir.path().join("mid/a/Cargo.toml")).unwrap();
    let document = parse_manifest(Path::new("mid/a/Cargo.toml"), &text).unwrap();
    let error = enclosing_workspace_manifest(dir.path(), Path::new("mid/a/Cargo.toml"), &document).unwrap_err();
    assert!(error.to_string().contains("mid/Cargo.toml"), "{error}");
}

#[test]
fn package_workspace_must_stay_inside_the_root_and_name_a_workspace() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "outside/Cargo.toml",
        &package("o", "workspace = \"../..\"\n"),
    );
    write(dir.path(), "plain/Cargo.toml", &package("p", ""));
    write(
        dir.path(),
        "named/Cargo.toml",
        &package("n", "workspace = \"../plain\"\n"),
    );
    for member in ["outside/Cargo.toml", "named/Cargo.toml"] {
        let text = fs::read_to_string(dir.path().join(member)).unwrap();
        let document = parse_manifest(Path::new(member), &text).unwrap();
        let error = enclosing_workspace_manifest(dir.path(), Path::new(member), &document).unwrap_err();
        assert!(error.to_string().contains("package.workspace"), "{member}: {error}");
    }
}

#[test]
fn candidates_are_every_ancestor_manifest_and_the_named_workspace() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "w/Cargo.toml", "[workspace]\n");
    write(dir.path(), "p/q/Cargo.toml", &package("q", "workspace = \"../../w\"\n"));
    let candidates = workspace_manifest_candidates(dir.path(), Path::new("p/q/Cargo.toml")).unwrap();
    assert_eq!(
        candidates,
        [
            PathBuf::from("Cargo.toml"),
            PathBuf::from("p/Cargo.toml"),
            PathBuf::from("w/Cargo.toml")
        ]
    );
}
