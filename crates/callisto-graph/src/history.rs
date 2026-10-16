//! Which package owned a shared name at an earlier commit, judged by the manifests that commit contained.

use std::collections::HashMap;
use std::path::Path;

use callisto_manifests::read_identity;
use callisto_model::vcs::GitAccess;
use callisto_model::{normalize_pypi_project_name, Ecosystem, ManifestFormat, Package};

use crate::error::GraphError;

/// The native name each manifest declared at a commit, for every commit already read.
#[derive(Default)]
pub(crate) struct Rosters(HashMap<String, Vec<(Ecosystem, String)>>);

impl Rosters {
    fn at(&mut self, git: &GitAccess<'_>, rev: &str) -> Result<&[(Ecosystem, String)], GraphError> {
        if !self.0.contains_key(rev) {
            self.0.insert(rev.to_string(), read_roster(git, rev)?);
        }
        Ok(&self.0[rev])
    }
}

fn read_roster(git: &GitAccess<'_>, rev: &str) -> Result<Vec<(Ecosystem, String)>, GraphError> {
    let mut roster = Vec::new();
    for path in git.files_at(rev)? {
        let Some(format) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(manifest_format)
        else {
            continue;
        };
        let Some(text) = git.read_at(rev, &path)? else {
            continue;
        };
        // A manifest that was broken at `rev` simply names nobody.
        let Some(name) = read_identity(format, &text, &path)
            .ok()
            .and_then(|identity| identity.name)
        else {
            continue;
        };
        let ecosystem = format.ecosystem();
        roster.push((ecosystem, name));
    }
    Ok(roster)
}

fn manifest_format(file_name: &str) -> Option<ManifestFormat> {
    match file_name {
        "Cargo.toml" => Some(ManifestFormat::CargoToml),
        "package.json" => Some(ManifestFormat::PackageJson),
        "pyproject.toml" => Some(ManifestFormat::PyprojectToml),
        _ => None,
    }
}

fn names(ecosystem: Ecosystem, candidate: &str, declared: &str) -> bool {
    match ecosystem {
        Ecosystem::Pypi => normalize_pypi_project_name(candidate) == normalize_pypi_project_name(declared),
        _ => candidate == declared,
    }
}

/// The one package among `candidates` that owned their shared bare name at `rev`, if exactly one did.
///
/// A candidate whose current first manifest path exists at `rev` owns it. When none does, the package moved or changed
/// its primary manifest, so ownership is whichever package had a manifest of one of its ecosystems declaring that name.
pub(crate) fn sole_owner_at<'p>(
    git: &GitAccess<'_>,
    rev: &str,
    candidates: &[&'p Package],
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
    let roster = rosters.at(git, rev)?;
    let mut declaring = candidates.iter().filter(|pkg| {
        pkg.canonical_manifests().any(|manifest| {
            let ecosystem = manifest.ecosystem();
            roster
                .iter()
                .any(|(declared_in, declared)| *declared_in == ecosystem && names(ecosystem, pkg.id.name(), declared))
        })
    });
    let first = declaring.next();
    Ok(if declaring.next().is_some() {
        None
    } else {
        first.copied()
    })
}
