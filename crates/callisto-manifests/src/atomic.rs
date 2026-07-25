use std::io::{self, Write};
use std::path::Path;
use tempfile::NamedTempFile;

pub fn atomic_write(path: &Path, content: &str) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(content.as_bytes())?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
