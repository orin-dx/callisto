use std::io::{self, Write};
use std::path::Path;
use tempfile::NamedTempFile;

/// Trait for durable changeset and manifest storage operations.
pub trait ChangesetStorage {
    /// Writes content atomically with parent and grandparent directory journal flushing.
    fn atomic_write_durable(&self, content: &str) -> io::Result<()>;
}

impl ChangesetStorage for Path {
    fn atomic_write_durable(&self, content: &str) -> io::Result<()> {
        atomic_write(self, content)
    }
}

pub fn atomic_write(path: &Path, content: &str) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    // Fsync grandparent directory if present
    if let Some(grandparent) = parent.parent() {
        if let Ok(gp_file) = std::fs::File::open(grandparent) {
            let _res = gp_file.sync_all();
        }
    }

    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(content.as_bytes())?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;

    if let Ok(parent_file) = std::fs::File::open(parent) {
        let _res = parent_file.sync_all();
    }
    Ok(())
}
