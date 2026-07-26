use std::path::PathBuf;

use crate::{DepKind, ManifestFormat, ManifestRole, PackageId, VersionGrammar, VersionParseError};

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelError {
    #[error("path `{path}` is absolute; callisto-model paths are workspace-root-relative")]
    AbsolutePath { path: PathBuf },

    #[error("path `{path}` is not valid UTF-8; callisto serializes paths into its JSON contract")]
    NonUtf8Path { path: PathBuf },

    #[error("path `{path}` attempts to traverse outside the workspace root")]
    PathTraversal { path: PathBuf },

    #[error("`{raw}` is not a valid 40-character hexadecimal commit sha")]
    InvalidCommitSha { raw: String, reason: String },

    #[error("manifest role {role:?} is not valid for format {format:?}")]
    InvalidRoleForFormat { role: String, format: String },

    #[error("package `{package}` has no canonical manifest; at least one is required")]
    NoCanonicalManifest { package: String },

    #[error("package `{package}` has canonical manifests in disagreeing version grammars ({grammars:?}); its version of record has no single grammar")]
    MixedVersionGrammars {
        package: String,
        grammars: Vec<VersionGrammar>,
    },

    #[error("package identity `{raw}` has ecosystem prefix `{prefix}` but no name after it")]
    EmptyNameAfterPrefix { raw: String, prefix: String },
}

impl ModelError {
    pub fn invalid_role_for_format(role: &ManifestRole, format: &ManifestFormat) -> Self {
        ModelError::InvalidRoleForFormat {
            role: format!("{role:?}"),
            format: format!("{format:?}"),
        }
    }

    pub fn no_canonical_manifest(package: &PackageId) -> Self {
        ModelError::NoCanonicalManifest {
            package: package.display_name(),
        }
    }

    pub fn mixed_version_grammars(package: &PackageId, grammars: Vec<VersionGrammar>) -> Self {
        ModelError::MixedVersionGrammars {
            package: package.display_name(),
            grammars,
        }
    }
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ManifestError {
    #[error("failed to read `{path}`: {message}")]
    Read { path: PathBuf, message: String },

    #[error("failed to write `{path}`: {message}")]
    Write { path: PathBuf, message: String },

    #[error("`{path}` is not valid {format:?}: {message}")]
    Parse {
        path: PathBuf,
        format: ManifestFormat,
        message: String,
    },

    #[error("`{path}` has no `{field}` field")]
    MissingField { path: PathBuf, field: &'static str },

    #[error("`{path}` declares `{raw}` as its version, which is invalid: {source}")]
    InvalidVersion {
        path: PathBuf,
        raw: String,
        #[source]
        source: VersionParseError,
    },

    #[error("`{path}` inherits `{key}` from the workspace root; write the root manifest instead")]
    WorkspaceInherited { path: PathBuf, key: String },

    #[error("`{path}` ({format:?}) is not a supported write target: {reason}")]
    ReadOnlyFormat {
        path: PathBuf,
        format: ManifestFormat,
        reason: &'static str,
    },

    #[error("`{path}` has no `{kind:?}` dependency named `{name}`")]
    DependencyNotFound {
        path: PathBuf,
        name: String,
        kind: DepKind,
    },

    #[error("operation `{operation}` is not supported for `{path}` ({format:?})")]
    UnsupportedOperation {
        path: PathBuf,
        format: ManifestFormat,
        operation: &'static str,
    },

    #[error("format-preserving write of `{path}` would not round-trip: {message}")]
    FormattingNotPreserved { path: PathBuf, message: String },
}
