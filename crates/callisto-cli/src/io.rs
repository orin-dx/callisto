//! The CLI's one way to read JSON inputs and to write output files.

use std::path::{Path, PathBuf};

use callisto_model::ApplyPermit;
use serde::de::DeserializeOwned;

use crate::error::CliError;

fn io_error(source: std::io::Error, path: &Path) -> CliError {
    CliError::Io {
        source,
        path: Some(path.to_path_buf()),
    }
}

/// Reads `arg` as JSON: `-` is stdin, a value starting with `{` is inline JSON, anything else is a file path.
/// A parse failure is `E281` naming `flag`.
pub fn read_json_input<T: DeserializeOwned>(flag: &'static str, arg: &str) -> Result<T, CliError> {
    let text = if arg == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
        buf
    } else if arg.trim_start().starts_with('{') {
        arg.to_owned()
    } else {
        std::fs::read_to_string(arg).map_err(|source| io_error(source, Path::new(arg)))?
    };
    serde_json::from_str(&text).map_err(|error| CliError::ReleasePrArgJsonInvalid {
        flag,
        detail: error.to_string(),
    })
}

/// Reads the JSON file at `path`. A parse failure is `E238` naming the file.
pub fn read_json_file<T: DeserializeOwned>(path: &Path) -> Result<T, CliError> {
    let text = std::fs::read_to_string(path).map_err(|source| io_error(source, path))?;
    serde_json::from_str(&text).map_err(|error| CliError::ReleaseJsonInvalid {
        path: path.display().to_string(),
        detail: error.to_string(),
    })
}

/// Which output flag a file is written for; it picks the existing dry-run refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputKind {
    /// `release artifact-manifest --out`.
    ArtifactManifest,
    /// `release plan --out`.
    Intent,
    /// `release execute --receipt`.
    Receipt,
    /// `release-pr commit-plan --out`.
    CommitPlan,
}

impl OutputKind {
    fn refusal(self) -> CliError {
        match self {
            Self::ArtifactManifest => CliError::ReleaseArtifactManifestDryRun,
            Self::Intent => CliError::ReleasePlanDryRun,
            Self::Receipt => CliError::ReleaseExecuteDryRun,
            Self::CommitPlan => CliError::ReleasePrCommitPlanDryRun,
        }
    }
}

/// An output file the run may write, held until the caller decides the run succeeded.
#[must_use = "an output file is only written by `commit`"]
#[derive(Debug)]
pub struct OutputFile {
    path: PathBuf,
    permit: ApplyPermit,
}

impl OutputFile {
    /// Refuses `--dry-run` with the flag's own error, before anything is written.
    pub fn new(path: PathBuf, dry_run: bool, kind: OutputKind) -> Result<Self, CliError> {
        let permit = ApplyPermit::granted_unless_dry_run(dry_run).ok_or_else(|| kind.refusal())?;
        Ok(Self { path, permit })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn permit(&self) -> ApplyPermit {
        self.permit
    }

    /// Fails now if the destination cannot be written, so a long run does not fail at the end.
    pub fn probe(&self) -> Result<(), CliError> {
        callisto_model::atomic::probe_atomic_write(&self.path, &self.permit)
            .map_err(|source| io_error(source, &self.path))
    }

    /// Atomically writes `content` as pretty JSON plus a newline.
    pub fn commit(self, permit: &ApplyPermit, content: &impl serde::Serialize) -> Result<(), CliError> {
        let text = serde_json::to_string_pretty(content).expect("output serializes") + "\n";
        callisto_model::atomic::atomic_write(&self.path, &text, permit).map_err(|source| io_error(source, &self.path))
    }
}
