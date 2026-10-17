use super::*;
use callisto_model::VersionGrammar;
use sha2::{Digest, Sha256};

fn version(raw: &str) -> Version {
    Version::parse(raw, VersionGrammar::Pep440).unwrap()
}

/// A built set holding `files` (name and content), as `from_dir` would inventory them.
fn set_of(files: &[(&str, &str)]) -> BuiltSet {
    let dir = tempfile::tempdir().unwrap();
    for (name, content) in files {
        std::fs::write(dir.path().join(name), content).unwrap();
    }
    BuiltSet::from_dir(dir, "My.Pkg", &version("1.0.0")).unwrap()
}

fn set() -> BuiltSet {
    set_of(&[
        ("my_pkg-1.0.0-py3-none-any.whl", "wheel bytes"),
        ("my_pkg-1.0.0.tar.gz", "sdist bytes"),
    ])
}

fn sha256(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

fn remote(filename: &str, sha256: Option<&str>, yanked: bool) -> PypiSimpleFile {
    PypiSimpleFile {
        filename: filename.to_string(),
        yanked: yanked.then(|| serde_json::Value::String(String::new())),
        hashes: sha256
            .map(|digest| HashMap::from([("sha256".to_string(), digest.to_string())]))
            .unwrap_or_default(),
    }
}

#[test]
fn a_filename_splits_into_its_name_and_version() {
    assert_eq!(
        dist_filename_parts("my_pkg-1.0.0-py3-none-any.whl"),
        Some(("my_pkg", "1.0.0"))
    );
    assert_eq!(dist_filename_parts("my-pkg-1.0.0.tar.gz"), Some(("my-pkg", "1.0.0")));
    assert_eq!(dist_filename_parts("my_pkg-1.0.0.egg"), None);
    assert_eq!(dist_filename_parts("README.md"), None);
}

#[test]
fn the_inventory_holds_only_this_packages_files_at_this_version() {
    let set = set_of(&[
        ("my_pkg-1.0.0-py3-none-any.whl", "wheel"),
        ("my_pkg-1.0.0.tar.gz", "sdist"),
        ("my_pkg-1.0.1-py3-none-any.whl", "another version"),
        ("my_pkg-1.0.1.tar.gz", "another version"),
        ("other_pkg-1.0.0-py3-none-any.whl", "another package"),
        ("notes.txt", "not a distribution"),
    ]);
    let names: Vec<_> = set.files.iter().map(|file| file.filename.as_str()).collect();
    assert_eq!(names, ["my_pkg-1.0.0-py3-none-any.whl", "my_pkg-1.0.0.tar.gz"]);
    assert_eq!(set.files[0].sha256, sha256("wheel"));
}

#[test]
fn a_build_without_a_wheel_or_an_sdist_is_an_error() {
    for (files, missing) in [
        (vec![("my_pkg-1.0.0.tar.gz", "sdist")], "wheel"),
        (vec![("my_pkg-1.0.0-py3-none-any.whl", "wheel")], "sdist"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        for (name, content) in files {
            std::fs::write(dir.path().join(name), content).unwrap();
        }
        let error = BuiltSet::from_dir(dir, "my-pkg", &version("1.0.0")).unwrap_err();
        assert!(
            matches!(error, GraphError::PypiBuildOutputIncomplete { missing: found, .. } if found == missing),
            "{error:?}"
        );
    }
}

#[test]
fn an_index_with_every_declared_file_is_complete() {
    let set = set();
    let held = [
        remote("my_pkg-1.0.0-py3-none-any.whl", None, false),
        remote("my_pkg-1.0.0.tar.gz", None, false),
    ];
    let comparison = compare(&set, &held);
    assert!(comparison.is_complete(), "{comparison:?}");
    assert!(comparison.extras.is_empty());
}

#[test]
fn a_declared_file_the_index_lacks_is_missing() {
    let set = set();
    let held = [remote("my_pkg-1.0.0-py3-none-any.whl", None, false)];
    let comparison = compare(&set, &held);
    assert_eq!(comparison.missing, ["my_pkg-1.0.0.tar.gz"]);
    assert!(!comparison.is_complete());
}

#[test]
fn a_declared_file_the_index_holds_yanked_is_flagged() {
    let set = set();
    let held = [
        remote("my_pkg-1.0.0-py3-none-any.whl", None, true),
        remote("my_pkg-1.0.0.tar.gz", None, true),
    ];
    let comparison = compare(&set, &held);
    assert_eq!(comparison.yanked.len(), 2);
    assert!(!comparison.is_complete());
}

/// A rebuild need not match an earlier attempt's bytes, so a file this run did not upload is trusted by name.
#[test]
fn a_digest_is_compared_only_for_files_this_run_uploaded() {
    let set = set();
    let held = [
        remote("my_pkg-1.0.0-py3-none-any.whl", Some("0".repeat(64).as_str()), false),
        remote("my_pkg-1.0.0.tar.gz", Some(sha256("sdist bytes").as_str()), false),
    ];
    assert!(compare(&set, &held).is_complete());

    set.mark_uploaded(&[&set.files[0]]);
    let comparison = compare(&set, &held);
    assert_eq!(comparison.differing, ["my_pkg-1.0.0-py3-none-any.whl"]);
    assert!(!comparison.is_complete());
}

#[test]
fn an_uploaded_file_with_the_same_digest_is_complete() {
    let set = set();
    set.mark_uploaded(&[&set.files[0], &set.files[1]]);
    let held = [
        remote(
            "my_pkg-1.0.0-py3-none-any.whl",
            Some(sha256("wheel bytes").as_str()),
            false,
        ),
        remote("my_pkg-1.0.0.tar.gz", Some(sha256("sdist bytes").as_str()), false),
    ];
    assert!(compare(&set, &held).is_complete());
}

/// An index that serves no digests cannot disagree.
#[test]
fn an_uploaded_file_without_an_index_digest_is_not_flagged() {
    let set = set();
    set.mark_uploaded(&[&set.files[0], &set.files[1]]);
    let held = [
        remote("my_pkg-1.0.0-py3-none-any.whl", None, false),
        remote("my_pkg-1.0.0.tar.gz", None, false),
    ];
    assert!(compare(&set, &held).is_complete());
}

#[test]
fn files_the_build_did_not_declare_are_reported_but_do_not_block() {
    let set = set();
    let held = [
        remote("my_pkg-1.0.0-py3-none-any.whl", None, false),
        remote("my_pkg-1.0.0.tar.gz", None, false),
        remote("my_pkg-1.0.0-cp312-cp312-manylinux_2_28_x86_64.whl", None, false),
    ];
    let comparison = compare(&set, &held);
    assert_eq!(
        comparison.extras,
        ["my_pkg-1.0.0-cp312-cp312-manylinux_2_28_x86_64.whl"]
    );
    assert!(comparison.is_complete());
}

#[test]
fn a_set_is_built_once_per_package_directory_and_version() {
    let sets = PypiFileSets::default();
    let mut builds = 0;
    let key = (PathBuf::from("packages/my-pkg"), "1.0.0".to_string());
    for _ in 0..2 {
        sets.get_or_build(key.clone(), || {
            builds += 1;
            Ok(set())
        })
        .unwrap();
    }
    assert_eq!(builds, 1);
    sets.get_or_build((PathBuf::from("packages/other"), "1.0.0".to_string()), || {
        builds += 1;
        Ok(set())
    })
    .unwrap();
    assert_eq!(builds, 2);
}
