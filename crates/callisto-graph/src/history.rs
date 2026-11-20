//! Which package owned a shared name at an earlier commit, judged by the manifests that commit contained.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use callisto_manifests::read_identity;
use callisto_model::vcs::{GitAccess, Pathspec};
use callisto_model::{Ecosystem, ManifestFormat, Package};

use crate::commands::git_batch::{batch_blobs, BatchBlob};
use crate::error::GraphError;
use crate::IdentityIndex;

/// The ecosystems with a manifest declaring a name at a revision, for every (revision, name) already read, and the
/// manifest texts read so far.
#[derive(Default)]
pub(crate) struct Rosters {
    declaring: HashMap<(String, String), BTreeSet<Ecosystem>>,
    texts: HashMap<(String, PathBuf), Option<String>>,
}

impl Rosters {
    /// Reads every `(rev, path)` not read yet in one `git cat-file --batch`, so the lookups that follow cost no process.
    ///
    /// A failed batch is not an error: the lookups then read one at a time, as they always could.
    pub(crate) fn prefetch(&mut self, git: &GitAccess<'_>, wanted: &[(String, PathBuf)]) {
        let missing: Vec<&(String, PathBuf)> = wanted.iter().filter(|key| !self.texts.contains_key(*key)).collect();
        let objects: Vec<String> = missing
            .iter()
            .map(|(rev, path)| format!("{rev}:./{}", path.display()))
            .collect();
        let Ok(blobs) = batch_blobs(git.runner(), git.root(), &objects) else {
            return;
        };
        for (key, blob) in missing.into_iter().zip(blobs) {
            let text = match blob {
                BatchBlob::Blob(text) => Some(text),
                BatchBlob::Missing => None,
            };
            self.texts.insert(key.clone(), text);
        }
    }

    fn manifest_text(&mut self, git: &GitAccess<'_>, rev: &str, path: &Path) -> Result<Option<String>, GraphError> {
        let key = (rev.to_owned(), path.to_path_buf());
        if let Some(known) = self.texts.get(&key) {
            return Ok(known.clone());
        }
        let text = git.read_at(rev, path)?;
        self.texts.insert(key, text.clone());
        Ok(text)
    }

    fn declaring_at(&mut self, git: &GitAccess<'_>, rev: &str, name: &str) -> Result<BTreeSet<Ecosystem>, GraphError> {
        let key = (rev.to_string(), name.to_string());
        if let Some(known) = self.declaring.get(&key) {
            return Ok(known.clone());
        }
        let mut declaring = BTreeSet::new();
        for (ecosystem, format, file_name) in MANIFESTS {
            if declares(git, rev, name, format, file_name)? {
                declaring.insert(ecosystem);
            }
        }
        self.declaring.insert(key, declaring.clone());
        Ok(declaring)
    }
}

const MANIFESTS: [(Ecosystem, ManifestFormat, &str); 3] = [
    (Ecosystem::Cargo, ManifestFormat::CargoToml, "Cargo.toml"),
    (Ecosystem::Npm, ManifestFormat::PackageJson, "package.json"),
    (Ecosystem::Pypi, ManifestFormat::PyprojectToml, "pyproject.toml"),
];

/// Whether `rev` held a manifest named `file_name` declaring exactly `name`, the spelling a default tag carries; only
/// manifests mentioning it are read.
fn declares(
    git: &GitAccess<'_>,
    rev: &str,
    name: &str,
    format: ManifestFormat,
    file_name: &str,
) -> Result<bool, GraphError> {
    for path in git.files_containing_at(rev, name, &[Pathspec::named_anywhere(file_name)])? {
        let Some(text) = git.read_at(rev, &path)? else {
            continue;
        };
        // A manifest that was broken at `rev` simply names nobody.
        let declared = read_identity(format, &text, &path)
            .ok()
            .and_then(|identity| identity.name);
        if declared.is_some_and(|declared| declared == name) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether `pkg`'s current first manifest held a manifest declaring `name` at `rev`.
fn first_manifest_declares_at(
    git: &GitAccess<'_>,
    rev: &str,
    pkg: &Package,
    name: &str,
    rosters: &mut Rosters,
) -> Result<bool, GraphError> {
    let Some(manifest) = pkg.canonical_manifests().next() else {
        return Ok(false);
    };
    let path = Path::new(&manifest.path);
    let Some(text) = rosters.manifest_text(git, rev, path)? else {
        return Ok(false);
    };
    // A manifest that was broken at `rev` simply names nobody.
    let declared = read_identity(manifest.format, &text, path)
        .ok()
        .and_then(|identity| identity.name);
    Ok(declared.as_deref() == Some(name))
}

/// The one package among `candidates` that owned the bare name `name` at `rev`, if exactly one did.
///
/// A candidate whose current first manifest path held a manifest declaring `name` at `rev` owns it. When none does,
/// the package moved or changed its primary manifest, so ownership is whichever package has a manifest named `name`
/// today in an ecosystem where `rev` also held a manifest named `name`.
pub(crate) fn sole_owner_at<'p>(
    git: &GitAccess<'_>,
    rev: &str,
    name: &str,
    candidates: &[&'p Package],
    identity: &IdentityIndex,
    rosters: &mut Rosters,
) -> Result<Option<&'p Package>, GraphError> {
    let mut owner = None;
    for pkg in candidates {
        if first_manifest_declares_at(git, rev, pkg, name, rosters)? {
            if owner.is_some() {
                return Ok(None);
            }
            owner = Some(*pkg);
        }
    }
    if owner.is_some() {
        return Ok(owner);
    }
    let declaring = rosters.declaring_at(git, rev, name)?;
    let mut holders = candidates.iter().filter(|pkg| {
        declaring.iter().any(|ecosystem| {
            identity
                .native_name(&pkg.id, *ecosystem)
                .is_some_and(|native| native == name)
        })
    });
    let first = holders.next();
    Ok(if holders.next().is_some() { None } else { first.copied() })
}

/// Whether `pkg` can be the package that held the bare name `name` at `rev`.
///
/// False only on evidence of another owner: `rev` held a manifest named `name`, but not in any ecosystem where `pkg`
/// is named `name` today. A revision with no manifest named `name` is no evidence either way.
pub(crate) fn may_have_held_name_at(
    git: &GitAccess<'_>,
    rev: &str,
    name: &str,
    pkg: &Package,
    identity: &IdentityIndex,
    rosters: &mut Rosters,
) -> Result<bool, GraphError> {
    if first_manifest_declares_at(git, rev, pkg, name, rosters)? {
        return Ok(true);
    }
    let declaring = rosters.declaring_at(git, rev, name)?;
    if declaring.is_empty() {
        return Ok(true);
    }
    Ok(declaring.iter().any(|ecosystem| {
        identity
            .native_name(&pkg.id, *ecosystem)
            .is_some_and(|native| native == name)
    }))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use callisto_fixtures::git::{init_repo, GitRunner};
    use callisto_model::vcs::GitAccess;
    use callisto_model::Package;

    use super::*;
    use crate::resolver::DependencyResolver;

    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}");
    }

    fn write(root: &Path, rel: &str, text: &str) {
        std::fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
        std::fs::write(root.join(rel), text).unwrap();
    }

    fn commit(root: &Path, message: &str) {
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", message]);
    }

    fn pyproject(name: &str) -> String {
        format!("[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n")
    }

    fn cargo_toml(name: &str) -> String {
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")
    }

    /// `a/` holds PyPI `my_pkg` and `c/` holds Cargo `foo_bar` at the first commit; at the second the PyPI manifest
    /// has moved to `b/`.
    fn repository() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        init_repo(root);
        write(root, "a/pyproject.toml", &pyproject("my_pkg"));
        write(root, "c/Cargo.toml", &cargo_toml("foo_bar"));
        commit(root, "first");
        git(root, &["mv", "a", "b"]);
        commit(root, "move");
        dir
    }

    #[test]
    fn a_manifest_declares_only_the_literal_name() {
        let dir = repository();
        let git = GitAccess::new(dir.path(), &GitRunner);
        let declares = |name, format, file| declares(&git, "HEAD~1", name, format, file).unwrap();
        assert!(declares("my_pkg", ManifestFormat::PyprojectToml, "pyproject.toml"));
        assert!(!declares("my-pkg", ManifestFormat::PyprojectToml, "pyproject.toml"));
        assert!(declares("foo_bar", ManifestFormat::CargoToml, "Cargo.toml"));
        assert!(!declares("foo-bar", ManifestFormat::CargoToml, "Cargo.toml"));
    }

    #[test]
    fn ownership_of_a_bare_name_follows_the_literal_spelling() {
        let dir = repository();
        let locator = crate::locate::IgnoreWalkLocator::new(dir.path());
        let ws = crate::Workspace::load(dir.path().to_path_buf(), &locator, &GitRunner).expect("load");
        let git = GitAccess::new(dir.path(), &GitRunner);
        let packages: Vec<&Package> = ws.graph.packages().collect();
        let python = *packages.iter().find(|p| p.id.name() == "my_pkg").expect("pypi package");
        let cargo = *packages
            .iter()
            .find(|p| p.id.name() == "foo_bar")
            .expect("cargo package");
        let mut rosters = Rosters::default();

        // The package's own manifest path held the name at the second commit.
        assert!(first_manifest_declares_at(&git, "HEAD", python, "my_pkg", &mut rosters).unwrap());
        assert!(!first_manifest_declares_at(&git, "HEAD", python, "my-pkg", &mut rosters).unwrap());
        assert!(first_manifest_declares_at(&git, "HEAD", cargo, "foo_bar", &mut rosters).unwrap());
        assert!(!first_manifest_declares_at(&git, "HEAD", cargo, "foo-bar", &mut rosters).unwrap());

        // At the first commit the PyPI manifest sat elsewhere, so ownership falls back to the declaring roster.
        let both = [python, cargo];
        let owner = sole_owner_at(&git, "HEAD~1", "my_pkg", &both, &ws.identity, &mut rosters).unwrap();
        assert_eq!(owner.map(|p| p.id.clone()), Some(python.id.clone()));
        let owner = sole_owner_at(&git, "HEAD~1", "foo_bar", &both, &ws.identity, &mut rosters).unwrap();
        assert_eq!(owner.map(|p| p.id.clone()), Some(cargo.id.clone()));

        assert!(may_have_held_name_at(&git, "HEAD~1", "my_pkg", python, &ws.identity, &mut rosters).unwrap());
        assert!(!may_have_held_name_at(&git, "HEAD~1", "my_pkg", cargo, &ws.identity, &mut rosters).unwrap());
        assert!(may_have_held_name_at(&git, "HEAD~1", "nobody", python, &ws.identity, &mut rosters).unwrap());
    }

    /// A default tag `foo-bar@` names the literal spelling: a commit holding a package `foo_bar` beside npm `foo-bar`
    /// made that tag for npm, so a package later renamed `foo-bar` does not inherit it, whatever its ecosystem.
    #[test]
    fn a_name_that_differs_only_by_separator_does_not_own_a_tag_of_the_literal_name() {
        for (manifest, earlier, later) in [
            ("c/Cargo.toml", cargo_toml("foo_bar"), cargo_toml("foo-bar")),
            ("c/pyproject.toml", pyproject("foo_bar"), pyproject("foo-bar")),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            init_repo(root);
            write(root, manifest, &earlier);
            write(root, "n/package.json", r#"{"name":"foo-bar","version":"1.0.0"}"#);
            commit(root, "both");
            git(root, &["rm", "-rq", "n"]);
            write(root, manifest, &later);
            commit(root, "npm removed, package renamed");
            let locator = crate::locate::IgnoreWalkLocator::new(root);
            let ws = crate::Workspace::load(root.to_path_buf(), &locator, &GitRunner).expect("load");
            let git = GitAccess::new(root, &GitRunner);
            let package = ws.graph.packages().next().expect("one package");
            let mut rosters = Rosters::default();

            let held = may_have_held_name_at(&git, "HEAD~1", "foo-bar", package, &ws.identity, &mut rosters).unwrap();
            assert!(!held, "{manifest}: the earlier commit's `foo-bar` was npm");
        }
    }
}
