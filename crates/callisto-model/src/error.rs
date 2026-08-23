use std::path::PathBuf;

use crate::{DepKind, ManifestFormat, ManifestRole, PackageId, VersionGrammar, VersionParseError};

#[derive(Debug, thiserror::Error, miette::Diagnostic, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelError {
    #[error("path `{path}` is absolute; callisto-model paths are workspace-root-relative")]
    #[diagnostic(code(E001), help("Use a relative path relative to the workspace root."))]
    AbsolutePath { path: PathBuf },

    #[error("path `{path}` is not valid UTF-8; callisto serializes paths into its JSON contract")]
    #[diagnostic(code(E002), help("Ensure all workspace file paths contain valid UTF-8 characters."))]
    NonUtf8Path { path: PathBuf },

    #[error("path `{path}` attempts to traverse outside the workspace root")]
    #[diagnostic(
        code(E003),
        help("Keep workspace file paths strictly within the workspace directory.")
    )]
    PathTraversal { path: PathBuf },

    #[error("`{raw}` is not a valid 40-character hexadecimal commit sha")]
    #[diagnostic(code(E004), help("Provide a valid full 40-character commit SHA."))]
    InvalidCommitSha { raw: String, reason: String },

    #[error("manifest role {role:?} is not valid for format {format:?}")]
    #[diagnostic(code(E005))]
    InvalidRoleForFormat { role: String, format: String },

    #[error("package `{package}` has no canonical manifest; at least one is required")]
    #[diagnostic(code(E006), help("Add a canonical manifest file for the package."))]
    NoCanonicalManifest { package: String },

    #[error("package `{package}` has canonical manifests in disagreeing version grammars ({grammars:?}); its version of record has no single grammar")]
    #[diagnostic(
        code(E007),
        help("Ensure all manifests for a package use consistent version grammars.")
    )]
    MixedVersionGrammars {
        package: String,
        grammars: Vec<VersionGrammar>,
    },

    #[error("package identity `{raw}` has ecosystem prefix `{prefix}` but no name after it")]
    #[diagnostic(code(E008), help("Include package name after ecosystem prefix."))]
    EmptyNameAfterPrefix { raw: String, prefix: String },

    #[error("manifest path `{path}` has unsupported format")]
    #[diagnostic(code(E009))]
    UnknownManifestFormat { path: PathBuf },
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

#[derive(Clone, Debug, thiserror::Error, miette::Diagnostic, PartialEq, Eq)]
#[non_exhaustive]
pub enum ManifestError {
    #[error("failed to read `{path}`: {message}")]
    #[diagnostic(code(E010), help("Check file permissions and path validity."))]
    Read { path: PathBuf, message: String },

    #[error("failed to write `{path}`: {message}")]
    #[diagnostic(code(E011), help("Check write permissions on target directory."))]
    Write { path: PathBuf, message: String },

    #[error("`{path}` is not valid {format:?}: {message}")]
    #[diagnostic(code(E012), help("Verify manifest syntax formatting."))]
    Parse {
        path: PathBuf,
        format: ManifestFormat,
        message: String,
    },

    #[error("`{path}` has no `{field}` field")]
    #[diagnostic(code(E013))]
    MissingField { path: PathBuf, field: &'static str },

    #[error("`{path}` declares `{raw}` as its version, which is invalid: {source}")]
    #[diagnostic(code(E014), help("Fix version string to follow valid semver or ecosystem grammar."))]
    InvalidVersion {
        path: PathBuf,
        raw: String,
        #[source]
        source: VersionParseError,
    },

    #[error("`{path}` inherits `{key}` from the workspace root; write the root manifest instead")]
    #[diagnostic(code(E015), help("Update workspace inheritance key in root manifest."))]
    WorkspaceInherited { path: PathBuf, key: String },

    #[error("`{path}` ({format:?}) is not a supported write target: {reason}")]
    #[diagnostic(code(E016))]
    ReadOnlyFormat {
        path: PathBuf,
        format: ManifestFormat,
        reason: &'static str,
    },

    #[error("`{path}` has no `{kind:?}` dependency named `{name}`")]
    #[diagnostic(code(E017))]
    DependencyNotFound { path: PathBuf, name: String, kind: DepKind },

    #[error("operation `{operation}` is not supported for `{path}` ({format:?})")]
    #[diagnostic(code(E018))]
    UnsupportedOperation {
        path: PathBuf,
        format: ManifestFormat,
        operation: &'static str,
    },

    #[error("format-preserving write of `{path}` would not round-trip: {message}")]
    #[diagnostic(code(E019), help("Ensure CST document retains formatting structure."))]
    FormattingNotPreserved { path: PathBuf, message: String },

    #[error("`{path}`: {message}")]
    #[diagnostic(code(E027))]
    InvariantViolation { path: PathBuf, message: String },

    #[error("`{path}` dependency `{name}` has a TOML value that is neither a string nor a table; refusing to silently no-op the rewrite")]
    #[diagnostic(
        code(E028),
        help("Fix the dependency's TOML shape to a plain string or a table before running callisto again.")
    )]
    UnrecognizedDependencyValue { path: PathBuf, name: String },
}
