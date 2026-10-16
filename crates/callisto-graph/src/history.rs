//! Which package owned a shared name at an earlier commit, judged by the manifests that commit contained.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use callisto_manifests::read_identity;
use callisto_model::vcs::GitAccess;
use callisto_model::{normalize_pypi_project_name, Ecosystem, ManifestFormat, Package};

use crate::error::GraphError;
use crate::IdentityIndex;

/// The ecosystems with a manifest declaring a name at a revision, for every (revision, name) already read.
#[derive(Default)]
pub(crate) struct Rosters(HashMap<(String, String), BTreeSet<Ecosystem>>);

impl Rosters {
    fn declaring_at(&mut self, git: &GitAccess<'_>, rev: &str, name: &str) -> Result<BTreeSet<Ecosystem>, GraphError> {
        let key = (rev.to_string(), name.to_string());
        if let Some(known) = self.0.get(&key) {
            return Ok(known.clone());
        }
        let mut declaring = BTreeSet::new();
        for (ecosystem, format, file_name) in MANIFESTS {
            if declares(git, rev, name, ecosystem, format, file_name)? {
                declaring.insert(ecosystem);
            }
        }
        self.0.insert(key, declaring.clone());
        Ok(declaring)
    }
}

const MANIFESTS: [(Ecosystem, ManifestFormat, &str); 3] = [
    (Ecosystem::Cargo, ManifestFormat::CargoToml, "Cargo.toml"),
    (Ecosystem::Npm, ManifestFormat::PackageJson, "package.json"),
    (Ecosystem::Pypi, ManifestFormat::PyprojectToml, "pyproject.toml"),
];

/// Whether `rev` held a manifest of `ecosystem` declaring `name`; only manifests mentioning it are read.
fn declares(
    git: &GitAccess<'_>,
    rev: &str,
    name: &str,
    ecosystem: Ecosystem,
    format: ManifestFormat,
    file_name: &str,
) -> Result<bool, GraphError> {
    // A normalized Python name may be spelled with other separators, so search its first word.
    let needle = match ecosystem {
        Ecosystem::Pypi => name
            .split(['-', '_', '.'])
            .find(|word| !word.is_empty())
            .unwrap_or(name),
        _ => name,
    };
    for path in git.files_containing_at(rev, needle, &[&format!(":(glob)**/{file_name}")])? {
        let Some(text) = git.read_at(rev, &path)? else {
            continue;
        };
        // A manifest that was broken at `rev` simply names nobody.
        let declared = read_identity(format, &text, &path)
            .ok()
            .and_then(|identity| identity.name);
        if declared.is_some_and(|declared| names_equal(ecosystem, &declared, name)) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn names_equal(ecosystem: Ecosystem, left: &str, right: &str) -> bool {
    match ecosystem {
        Ecosystem::Pypi => normalize_pypi_project_name(left) == normalize_pypi_project_name(right),
        _ => left == right,
    }
}

/// The one package among `candidates` that owned the bare name `name` at `rev`, if exactly one did.
///
/// A candidate whose current first manifest path exists at `rev` owns it. When none does, the package moved or changed
/// its primary manifest, so ownership is whichever package has a manifest named `name` today in an ecosystem where
/// `rev` also held a manifest named `name`.
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
        let Some(manifest) = pkg.canonical_manifests().next() else {
            continue;
        };
        if git.path_exists_at(rev, Path::new(&manifest.path))? {
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
                .is_some_and(|native| names_equal(*ecosystem, native, name))
        })
    });
    let first = holders.next();
    Ok(if holders.next().is_some() { None } else { first.copied() })
}
