//! Artifact-byte and GitHub-attestation verification for durable releases.
//!
//! A manifest is only a claim. This module verifies the actual on-disk bytes
//! and asks GitHub to validate the signing certificate against the policy
//! embedded in the immutable release intent before an executor can upload an
//! asset. It intentionally owns no upload side effect.

use std::{
    fs::{self, File},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use callisto_model::{ArtifactManifestV1, CommandRunner, ReleaseIntentV1};

use crate::GraphError;

const ATTESTATION_TIMEOUT: Duration = Duration::from_secs(120);

/// A manifest whose exact local bytes and GitHub provenance have been checked.
///
/// The constructor is private: callers can obtain this capability only with
/// [`verify_artifact_manifest`]. It borrows the manifest so an executor cannot
/// silently substitute a different manifest after verification.
#[derive(Debug)]
pub struct VerifiedArtifactManifest<'a> {
    manifest: &'a ArtifactManifestV1,
}

impl<'a> VerifiedArtifactManifest<'a> {
    pub fn manifest(&self) -> &'a ArtifactManifestV1 {
        self.manifest
    }
}

/// Verifies every manifest entry against the exact artifact directory.
///
/// The artifact directory and every candidate are canonicalized, and symbolic
/// links are rejected, so a manifest asset name cannot escape the supplied
/// build-output directory. Each asset is streamed once for a bounded-memory
/// SHA-256 and size comparison. `gh attestation verify` then validates the
/// repository, workflow path and commit, source commit, and hosted-runner
/// policy declared by the immutable artifact slot.
pub fn verify_artifact_manifest<'a, R: CommandRunner>(
    intent: &ReleaseIntentV1,
    manifest: &'a ArtifactManifestV1,
    artifact_root: &Path,
    runner: &R,
) -> Result<VerifiedArtifactManifest<'a>, GraphError> {
    manifest
        .validate_for_intent(intent)
        .map_err(|source| GraphError::ArtifactManifest { source })?;

    let root = canonical_artifact_root(artifact_root)?;
    for entry in &manifest.entries {
        let path = resolve_asset_path(&root, &entry.slot.asset_name)?;
        let file = File::open(&path).map_err(|error| GraphError::ArtifactRead {
            path: path.clone(),
            message: error.to_string(),
        })?;
        let (digest, length) =
            callisto_model::ArtifactDigest::from_reader(file).map_err(|error| GraphError::ArtifactRead {
                path: path.clone(),
                message: error.to_string(),
            })?;
        if digest != entry.digest || length != entry.byte_length {
            return Err(GraphError::ArtifactBytesMismatch { path });
        }

        verify_github_attestation(&path, entry, manifest, runner)?;
    }
    Ok(VerifiedArtifactManifest { manifest })
}

fn canonical_artifact_root(artifact_root: &Path) -> Result<PathBuf, GraphError> {
    let metadata = fs::metadata(artifact_root).map_err(|error| GraphError::ArtifactRead {
        path: artifact_root.to_path_buf(),
        message: error.to_string(),
    })?;
    if !metadata.is_dir() {
        return Err(GraphError::UnsafeArtifactPath {
            path: artifact_root.to_path_buf(),
            reason: "artifact root is not a directory",
        });
    }
    artifact_root.canonicalize().map_err(|error| GraphError::ArtifactRead {
        path: artifact_root.to_path_buf(),
        message: error.to_string(),
    })
}

fn resolve_asset_path(root: &Path, asset_name: &str) -> Result<PathBuf, GraphError> {
    let relative = Path::new(asset_name);
    if relative.components().count() != 1 || !matches!(relative.components().next(), Some(Component::Normal(_))) {
        return Err(GraphError::UnsafeArtifactPath {
            path: relative.to_path_buf(),
            reason: "asset name is not a single relative file name",
        });
    }
    let candidate = root.join(relative);
    let metadata = fs::symlink_metadata(&candidate).map_err(|error| GraphError::ArtifactRead {
        path: candidate.clone(),
        message: error.to_string(),
    })?;
    if metadata.file_type().is_symlink() {
        return Err(GraphError::UnsafeArtifactPath {
            path: candidate,
            reason: "symbolic links are not accepted as artifacts",
        });
    }
    if !metadata.is_file() {
        return Err(GraphError::UnsafeArtifactPath {
            path: candidate,
            reason: "artifact is not a regular file",
        });
    }
    let canonical = candidate.canonicalize().map_err(|error| GraphError::ArtifactRead {
        path: candidate.clone(),
        message: error.to_string(),
    })?;
    if !canonical.starts_with(root) {
        return Err(GraphError::UnsafeArtifactPath {
            path: canonical,
            reason: "artifact resolves outside artifact root",
        });
    }
    Ok(canonical)
}

fn verify_github_attestation<R: CommandRunner>(
    path: &Path,
    entry: &callisto_model::ArtifactManifestEntryV1,
    manifest: &ArtifactManifestV1,
    runner: &R,
) -> Result<(), GraphError> {
    let policy = &entry.slot.attestation_policy;
    let workflow = format!("{}/{}", policy.repository, policy.workflow_path);
    let path_argument = path.to_string_lossy();
    let source_commit = manifest.source_commit.as_str();
    let args = [
        "attestation",
        "verify",
        path_argument.as_ref(),
        "--repo",
        policy.repository.as_str(),
        "--signer-workflow",
        workflow.as_str(),
        "--signer-digest",
        policy.workflow_commit.as_str(),
        "--source-digest",
        source_commit,
        "--deny-self-hosted-runners",
    ];
    let output = runner
        .run_with_timeout(
            "gh",
            &args,
            path.parent().unwrap_or_else(|| Path::new(".")),
            ATTESTATION_TIMEOUT,
        )
        .map_err(|error| GraphError::ArtifactAttestation {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    if output.success() {
        Ok(())
    } else {
        Err(GraphError::ArtifactAttestation {
            path: path.to_path_buf(),
            message: "GitHub attestation verification failed".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, fs, path::Path};

    use callisto_model::{
        ArtifactDigest, ArtifactManifestEntryV1, ArtifactSlotId, CommandError, CommandOutput, Ecosystem,
        GitHubArtifactAttestationV1, ReleaseIntentV1, ReleasePackageId, Version, VersionGrammar,
    };
    use tempfile::tempdir;

    use super::*;

    #[derive(Default)]
    struct RecordingRunner {
        calls: std::sync::Mutex<Vec<(String, Vec<String>)>>,
        outputs: std::sync::Mutex<VecDeque<Result<CommandOutput, CommandError>>>,
    }

    impl CommandRunner for RecordingRunner {
        fn run(&self, program: &str, args: &[&str], _cwd: &Path) -> Result<CommandOutput, CommandError> {
            self.calls
                .lock()
                .unwrap()
                .push((program.to_owned(), args.iter().map(|arg| (*arg).to_owned()).collect()));
            self.outputs.lock().unwrap().pop_front().unwrap_or(Ok(CommandOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
            }))
        }
    }

    fn intent_with_slot(slot: ArtifactSlotId) -> ReleaseIntentV1 {
        use callisto_model::{
            ExecutionTrustProfileV1, ReleaseDecisionEntry, ReleaseDecisionV1, ReleaseInclusionReason,
            ReleaseInputSnapshotV1, ReleaseOperation, SourceIdentity,
        };
        let source = callisto_model::CommitSha::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        let snapshot = ReleaseInputSnapshotV1::new(SourceIdentity::GitCommit { sha: source.clone() }, vec![]).unwrap();
        let decision = ReleaseDecisionV1::new(vec![ReleaseDecisionEntry {
            package: slot.package.clone(),
            target_version: slot.version.clone(),
            reasons: vec![ReleaseInclusionReason::ExplicitSelection],
        }])
        .unwrap();
        let operation = ReleaseOperation::artifact_upload(slot.clone(), vec![]).unwrap();
        ReleaseIntentV1::new(
            decision,
            snapshot,
            ExecutionTrustProfileV1::GitCommit,
            vec![operation],
            vec![slot],
        )
        .unwrap()
    }

    fn manifest_for(intent: &ReleaseIntentV1, digest: ArtifactDigest, length: u64) -> ArtifactManifestV1 {
        let slot = intent.artifact_slots[0].clone();
        let attestation = GitHubArtifactAttestationV1 {
            repository: slot.attestation_policy.repository.clone(),
            workflow_path: slot.attestation_policy.workflow_path.clone(),
            workflow_commit: slot.attestation_policy.workflow_commit.clone(),
            subject_digest: digest.clone(),
            source_commit: match &intent.snapshot.source {
                callisto_model::SourceIdentity::GitCommit { sha } => sha.clone(),
                callisto_model::SourceIdentity::HermeticContent { .. } => panic!("test intent has a Git source"),
            },
        };
        ArtifactManifestV1::new(
            intent,
            vec![ArtifactManifestEntryV1 {
                slot,
                digest,
                byte_length: length,
                attestation,
            }],
        )
        .unwrap()
    }

    fn slot(asset_name: &str) -> ArtifactSlotId {
        let package = ReleasePackageId::new(Ecosystem::Npm, "package").unwrap();
        ArtifactSlotId::new(
            package,
            Version::parse("1.0.0", VersionGrammar::SemVer).unwrap(),
            "linux-x64",
            asset_name,
            "owner/repository",
            ".github/workflows/release.yml",
            callisto_model::CommitSha::parse("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn verifies_exact_bytes_and_pinned_github_policy() {
        let directory = tempdir().unwrap();
        let bytes = b"trusted artifact";
        fs::write(directory.path().join("artifact.tar.gz"), bytes).unwrap();
        let intent = intent_with_slot(slot("artifact.tar.gz"));
        let manifest = manifest_for(&intent, ArtifactDigest::from_bytes(bytes), bytes.len() as u64);
        let runner = RecordingRunner::default();

        let verified = verify_artifact_manifest(&intent, &manifest, directory.path(), &runner).unwrap();
        assert_eq!(verified.manifest(), &manifest);
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "gh");
        assert!(calls[0].1.windows(2).any(|args| args == ["--repo", "owner/repository"]));
        assert!(calls[0]
            .1
            .windows(2)
            .any(|args| args == ["--signer-workflow", "owner/repository/.github/workflows/release.yml"]));
        assert!(calls[0]
            .1
            .windows(2)
            .any(|args| args == ["--signer-digest", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"]));
        assert!(calls[0]
            .1
            .windows(2)
            .any(|args| args == ["--source-digest", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]));
        assert!(calls[0].1.contains(&"--deny-self-hosted-runners".to_owned()));
    }

    #[test]
    fn rejects_changed_artifact_without_attestation_lookup() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("artifact.tar.gz"), b"changed").unwrap();
        let intent = intent_with_slot(slot("artifact.tar.gz"));
        let manifest = manifest_for(&intent, ArtifactDigest::from_bytes(b"original"), 8);
        let runner = RecordingRunner::default();

        assert!(matches!(
            verify_artifact_manifest(&intent, &manifest, directory.path(), &runner),
            Err(GraphError::ArtifactBytesMismatch { .. })
        ));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_artifacts_without_attestation_lookup() {
        use std::os::unix::fs::symlink;
        let directory = tempdir().unwrap();
        let outside = tempdir().unwrap();
        fs::write(outside.path().join("artifact.tar.gz"), b"outside").unwrap();
        symlink(
            outside.path().join("artifact.tar.gz"),
            directory.path().join("artifact.tar.gz"),
        )
        .unwrap();
        let intent = intent_with_slot(slot("artifact.tar.gz"));
        let manifest = manifest_for(&intent, ArtifactDigest::from_bytes(b"outside"), 7);
        let runner = RecordingRunner::default();

        assert!(matches!(
            verify_artifact_manifest(&intent, &manifest, directory.path(), &runner),
            Err(GraphError::UnsafeArtifactPath { .. })
        ));
        assert!(runner.calls.lock().unwrap().is_empty());
    }
}
