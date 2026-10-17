//! The file set of one Python release: what a build produced, what the index holds, and what is missing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use callisto_model::{normalize_pypi_project_name, CommandError, Version};
use sha2::{Digest, Sha256};

use crate::error::GraphError;

/// One file entry of a PEP 691 JSON simple-index response: only the fields the release path reads.
#[derive(serde::Deserialize)]
pub(super) struct PypiSimpleFile {
    pub(super) filename: String,
    /// `false` when not yanked, or a (possibly empty) reason string when it is.
    #[serde(default)]
    pub(super) yanked: Option<serde_json::Value>,
    /// Digest by algorithm; an index that serves none leaves this empty.
    #[serde(default)]
    pub(super) hashes: HashMap<String, String>,
}

#[derive(serde::Deserialize)]
pub(super) struct PypiSimpleIndex {
    pub(super) files: Vec<PypiSimpleFile>,
}

impl PypiSimpleFile {
    pub(super) fn is_yanked(&self) -> bool {
        !matches!(self.yanked, None | Some(serde_json::Value::Bool(false)))
    }

    fn sha256(&self) -> Option<&str> {
        self.hashes.get("sha256").map(String::as_str)
    }
}

/// The name and version of a wheel or sdist filename; an sdist's last `-` splits them, as a version has no hyphen.
pub(super) fn dist_filename_parts(filename: &str) -> Option<(&str, &str)> {
    if let Some(stem) = filename.strip_suffix(".whl") {
        let mut segments = stem.split('-');
        return Some((segments.next()?, segments.next()?));
    }
    [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.Z", ".zip"]
        .into_iter()
        .find_map(|ext| filename.strip_suffix(ext))
        .and_then(|stem| stem.rsplit_once('-'))
}

fn is_wheel(filename: &str) -> bool {
    filename.ends_with(".whl")
}

fn is_sdist(filename: &str) -> bool {
    filename.ends_with(".tar.gz")
}

fn is_version(candidate: &str, version: &Version) -> bool {
    Version::parse(candidate, version.grammar())
        .is_ok_and(|candidate| candidate.compare(version) == Ok(std::cmp::Ordering::Equal))
}

/// One distribution file of a release: built by this run, or sealed by a build job.
#[derive(Debug)]
pub(super) struct BuiltFile {
    pub(super) filename: String,
    pub(super) path: PathBuf,
    pub(super) sha256: String,
}

/// The files of one release. A built set owns the temporary directory holding them; a sealed set points into the
/// verified artifact directory and compares every digest strictly.
#[derive(Debug)]
pub(super) struct BuiltSet {
    _dir: Option<tempfile::TempDir>,
    pub(super) files: Vec<BuiltFile>,
    sealed: bool,
    /// Files this run uploaded, whose index entry must then carry the same bytes.
    uploaded: Mutex<Vec<String>>,
}

fn io_error(error: std::io::Error) -> GraphError {
    GraphError::Command(CommandError::Io {
        program: "python -m build".to_string(),
        message: error.to_string(),
    })
}

/// Every wheel and sdist of `package_name` at `version` directly in `dir`, with its digest, by filename.
fn inventory(dir: &Path, package_name: &str, version: &Version) -> Result<Vec<BuiltFile>, GraphError> {
    let wanted = normalize_pypi_project_name(package_name);
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(io_error)? {
        let path = entry.map_err(io_error)?.path();
        let Some(filename) = path.file_name().and_then(|name| name.to_str()).map(str::to_owned) else {
            continue;
        };
        let belongs = dist_filename_parts(&filename).is_some_and(|(name, candidate)| {
            normalize_pypi_project_name(name) == wanted && is_version(candidate, version)
        });
        if !belongs {
            continue;
        }
        let sha256 = format!("{:x}", Sha256::digest(std::fs::read(&path).map_err(io_error)?));
        files.push(BuiltFile { filename, path, sha256 });
    }
    files.sort_by(|left, right| left.filename.cmp(&right.filename));
    for (present, missing) in [
        (files.iter().any(|file| is_wheel(&file.filename)), "wheel"),
        (files.iter().any(|file| is_sdist(&file.filename)), "sdist"),
    ] {
        if !present {
            return Err(GraphError::PypiBuildOutputIncomplete {
                package: package_name.to_string(),
                version: version.clone(),
                missing,
            });
        }
    }
    Ok(files)
}

impl BuiltSet {
    /// Inventories `dir`: every wheel and sdist of `package_name` at `version`, with its digest.
    pub(super) fn from_dir(dir: tempfile::TempDir, package_name: &str, version: &Version) -> Result<Self, GraphError> {
        let files = inventory(dir.path(), package_name, version)?;
        Ok(Self {
            _dir: Some(dir),
            files,
            sealed: false,
            uploaded: Mutex::new(Vec::new()),
        })
    }

    /// The files a build job sealed, already checked byte for byte against the manifest.
    pub(super) fn sealed(files: Vec<BuiltFile>, package_name: &str, version: &Version) -> Result<Self, GraphError> {
        let wanted = normalize_pypi_project_name(package_name);
        if let Some(stray) = files.iter().find(|file| {
            !dist_filename_parts(&file.filename).is_some_and(|(name, candidate)| {
                normalize_pypi_project_name(name) == wanted && is_version(candidate, version)
            })
        }) {
            return Err(GraphError::ReleaseInvariant {
                detail: format!(
                    "sealed file `{}` is not a distribution of {package_name} {}",
                    stray.filename,
                    version.render()
                ),
            });
        }
        Ok(Self {
            _dir: None,
            files,
            sealed: true,
            uploaded: Mutex::new(Vec::new()),
        })
    }

    /// Re-hashes the files about to be uploaded: a sealed file changed after verification must not reach the index.
    pub(super) fn recheck(&self, files: &[&BuiltFile]) -> Result<(), GraphError> {
        if !self.sealed {
            return Ok(());
        }
        for file in files {
            let metadata = std::fs::symlink_metadata(&file.path).map_err(io_error)?;
            let bytes = std::fs::read(&file.path).map_err(io_error)?;
            if !metadata.is_file() || format!("{:x}", Sha256::digest(bytes)) != file.sha256 {
                return Err(GraphError::ArtifactBytesMismatch {
                    path: file.path.clone(),
                });
            }
        }
        Ok(())
    }

    pub(super) fn mark_uploaded(&self, files: &[&BuiltFile]) {
        let mut uploaded = self.uploaded.lock().expect("uploaded list is never poisoned");
        uploaded.extend(files.iter().map(|file| file.filename.clone()));
    }

    fn uploaded(&self) -> Vec<String> {
        self.uploaded.lock().expect("uploaded list is never poisoned").clone()
    }
}

/// The sealed file set of every Python publish in `intent`, from the build output in `dir`.
pub(crate) fn seal_distributions(
    intent: &callisto_model::ReleaseIntentV1,
    dir: &Path,
) -> Result<Vec<callisto_model::DistributionSetV1>, GraphError> {
    let mut sets = Vec::new();
    for (package, rendered) in callisto_model::python_publishes(intent) {
        let version = intent
            .operations
            .iter()
            .map(|operation| operation.id())
            .find(|id| id.package == *package && id.version.render() == rendered)
            .map(|id| id.version.clone())
            .expect("a Python publish listed from the intent is in the intent");
        let files = inventory(dir, package.name(), &version)?
            .into_iter()
            .map(|file| {
                let byte_length = std::fs::metadata(&file.path).map_err(io_error)?.len();
                let digest = callisto_model::ArtifactDigest::parse(&file.sha256)
                    .expect("a SHA-256 hex digest is a canonical artifact digest");
                Ok(callisto_model::DistributionFileV1 {
                    filename: file.filename,
                    digest,
                    byte_length,
                })
            })
            .collect::<Result<Vec<_>, GraphError>>()?;
        sets.push(callisto_model::DistributionSetV1 {
            package: package.clone(),
            version: rendered,
            files,
        });
    }
    Ok(sets)
}

/// How the index's files for a release differ from the files a build declared.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Comparison<'a> {
    /// Declared files the index lacks.
    pub(super) missing: Vec<&'a str>,
    /// Declared files the index holds yanked.
    pub(super) yanked: Vec<&'a str>,
    /// Files whose index digest differs from the local file.
    pub(super) differing: Vec<&'a str>,
    /// Files the index holds for this release that no build declared.
    pub(super) extras: Vec<&'a str>,
}

impl Comparison<'_> {
    /// Whether every declared file is on the index, with nothing to flag about it.
    pub(super) fn is_complete(&self) -> bool {
        self.missing.is_empty() && self.yanked.is_empty() && self.differing.is_empty()
    }
}

/// Presence is by filename. A sealed set compares every digest; otherwise only files this run uploaded, as a
/// rebuild need not match an earlier attempt.
pub(super) fn compare<'a>(set: &'a BuiltSet, remote: &'a [PypiSimpleFile]) -> Comparison<'a> {
    let uploaded = set.uploaded();
    let mut comparison = Comparison::default();
    for file in &set.files {
        match remote.iter().find(|candidate| candidate.filename == file.filename) {
            None => comparison.missing.push(&file.filename),
            Some(held) if held.is_yanked() => comparison.yanked.push(&file.filename),
            Some(held) => {
                let unconfirmed = if set.sealed {
                    held.sha256() != Some(file.sha256.as_str())
                } else {
                    uploaded.contains(&file.filename) && held.sha256().is_some_and(|digest| digest != file.sha256)
                };
                if unconfirmed {
                    comparison.differing.push(&file.filename);
                }
            }
        }
    }
    comparison.extras = remote
        .iter()
        .map(|held| held.filename.as_str())
        .filter(|name| set.files.iter().all(|file| file.filename != *name))
        .collect();
    comparison
}

/// The built sets of one run, so a preflight and the publish after it share one build.
#[derive(Debug, Default)]
pub(crate) struct PypiFileSets(Mutex<HashMap<(PathBuf, String), Arc<BuiltSet>>>);

impl PypiFileSets {
    pub(super) fn get_or_build(
        &self,
        key: (PathBuf, String),
        build: impl FnOnce() -> Result<BuiltSet, GraphError>,
    ) -> Result<Arc<BuiltSet>, GraphError> {
        let mut sets = self.0.lock().expect("file set cache is never poisoned");
        if let Some(set) = sets.get(&key) {
            return Ok(Arc::clone(set));
        }
        let set = Arc::new(build()?);
        sets.insert(key, Arc::clone(&set));
        Ok(set)
    }
}

#[cfg(test)]
mod tests;
