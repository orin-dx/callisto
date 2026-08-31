//! Narrow, explicit-path persistence for durable release execution state.
//!
//! The store owns no release policy and does not derive paths from mutable
//! workspace configuration. Callers must supply the state path deliberately.

use std::path::{Path, PathBuf};

use callisto_model::{atomic::atomic_write, ApplyPermit, ReleaseExecutionStateV1, ReleaseIntentV1};

use crate::GraphError;

/// The only write seam used by [`ReleaseStateStore`]. Production writes use
/// Callisto's crash-safe atomic replacement primitive; tests can inject a
/// pre-rename failure without weakening that production path.
pub trait ReleaseStateWriter {
    fn write(&self, path: &Path, content: &str, permit: &ApplyPermit) -> std::io::Result<()>;
}

/// Production state writer backed by [`callisto_model::atomic::atomic_write`].
#[derive(Clone, Copy, Debug, Default)]
pub struct AtomicReleaseStateWriter;

impl ReleaseStateWriter for AtomicReleaseStateWriter {
    fn write(&self, path: &Path, content: &str, permit: &ApplyPermit) -> std::io::Result<()> {
        atomic_write(path, content, permit)
    }
}

/// Durable state store at one caller-chosen path.
#[derive(Debug)]
pub struct ReleaseStateStore<W = AtomicReleaseStateWriter> {
    path: PathBuf,
    writer: W,
}

impl ReleaseStateStore {
    /// Creates a production store at an explicit path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_writer(path, AtomicReleaseStateWriter)
    }
}

impl<W> ReleaseStateStore<W>
where
    W: ReleaseStateWriter,
{
    /// Creates a store with a narrow writer seam, primarily for deterministic
    /// failure tests. The path remains explicit and is never config-derived.
    pub fn with_writer(path: impl Into<PathBuf>, writer: W) -> Self {
        Self {
            path: path.into(),
            writer,
        }
    }

    /// Returns the caller-chosen durable state location.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads state if it exists and rejects any state not exactly bound to
    /// `intent`. A missing file is not evidence of a completed release.
    pub fn load(&self, intent: &ReleaseIntentV1) -> Result<Option<ReleaseExecutionStateV1>, GraphError> {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(GraphError::ReleaseStateRead {
                    path: self.path.clone(),
                    message: error.to_string(),
                });
            }
        };
        let state: ReleaseExecutionStateV1 =
            serde_json::from_str(&content).map_err(|error| GraphError::ReleaseStateDecode {
                path: self.path.clone(),
                message: error.to_string(),
            })?;
        state
            .validate_for_intent(intent)
            .map_err(|source| GraphError::ReleaseExecutionState { source })?;
        Ok(Some(state))
    }

    /// Loads existing state or atomically writes the exact pending roster.
    pub fn load_or_initialize(
        &self,
        intent: &ReleaseIntentV1,
        permit: &ApplyPermit,
    ) -> Result<ReleaseExecutionStateV1, GraphError> {
        if let Some(state) = self.load(intent)? {
            return Ok(state);
        }
        let state = ReleaseExecutionStateV1::pending(intent);
        self.save(intent, &state, permit)?;
        Ok(state)
    }

    /// Atomically persists only state proven to belong to `intent`.
    pub fn save(
        &self,
        intent: &ReleaseIntentV1,
        state: &ReleaseExecutionStateV1,
        permit: &ApplyPermit,
    ) -> Result<(), GraphError> {
        state
            .validate_for_intent(intent)
            .map_err(|source| GraphError::ReleaseExecutionState { source })?;
        let content = serde_json::to_string_pretty(state).expect("release state serialization is infallible") + "\n";
        self.writer
            .write(&self.path, &content, permit)
            .map_err(|error| GraphError::ReleaseStateWrite {
                path: self.path.clone(),
                message: error.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use callisto_model::{
        Ecosystem, ExecutionTrustProfileV1, ReleaseInputSnapshotV1, ReleaseOperation, ReleasePackageId, SourceIdentity,
        Version,
    };
    use tempfile::tempdir;

    use super::*;

    fn intent(sha: char) -> ReleaseIntentV1 {
        let package = ReleasePackageId::new(Ecosystem::Cargo, "demo").unwrap();
        let operation =
            ReleaseOperation::registry_publish(package, Version::semver(1, 0, 0), "crates", vec![]).unwrap();
        ReleaseIntentV1::new(
            ReleaseInputSnapshotV1::new(SourceIdentity::git_commit(sha.to_string().repeat(40)).unwrap(), vec![]),
            ExecutionTrustProfileV1::GitCommit,
            vec![operation],
        )
        .unwrap()
    }

    #[test]
    fn initializes_then_loads_exact_intent_bound_state() {
        let directory = tempdir().unwrap();
        let store = ReleaseStateStore::new(directory.path().join("state.json"));
        let intent = intent('a');
        let permit = ApplyPermit::force_for_tests();
        let initial = store.load_or_initialize(&intent, &permit).unwrap();
        assert_eq!(store.load(&intent).unwrap(), Some(initial));
        assert!(std::fs::read_to_string(store.path()).unwrap().contains("intentDigest"));
    }

    #[test]
    fn mismatched_intent_is_never_loaded_as_state() {
        let directory = tempdir().unwrap();
        let store = ReleaseStateStore::new(directory.path().join("state.json"));
        let permit = ApplyPermit::force_for_tests();
        store.load_or_initialize(&intent('a'), &permit).unwrap();
        assert!(matches!(
            store.load(&intent('b')),
            Err(GraphError::ReleaseExecutionState { .. })
        ));
    }

    #[derive(Clone, Copy, Debug)]
    struct FailBeforeRename;

    impl ReleaseStateWriter for FailBeforeRename {
        fn write(&self, _path: &Path, _content: &str, _permit: &ApplyPermit) -> io::Result<()> {
            Err(io::Error::other("injected pre-rename failure"))
        }
    }

    #[test]
    fn injected_pre_rename_failure_never_becomes_successful_state() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.json");
        let store = ReleaseStateStore::with_writer(&path, FailBeforeRename);
        let permit = ApplyPermit::force_for_tests();
        assert!(matches!(
            store.load_or_initialize(&intent('a'), &permit),
            Err(GraphError::ReleaseStateWrite { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(path).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn failed_replacement_preserves_the_last_durable_state() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.json");
        let permit = ApplyPermit::force_for_tests();
        let release_intent = intent('a');
        let production = ReleaseStateStore::new(&path);
        let pending = production.load_or_initialize(&release_intent, &permit).unwrap();

        let failing = ReleaseStateStore::with_writer(&path, FailBeforeRename);
        let operation = release_intent.operations[0].id().clone();
        let mut changed = pending.clone();
        changed.mark_attempting(&operation).unwrap();
        assert!(matches!(
            failing.save(&release_intent, &changed, &permit),
            Err(GraphError::ReleaseStateWrite { .. })
        ));
        assert_eq!(production.load(&release_intent).unwrap(), Some(pending));
    }
}
