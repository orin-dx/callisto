//! Source-scanning helpers shared by the architecture enforcement tests.

use std::fs;
use std::path::{Path, PathBuf};

pub fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR for this test binary is crates/callisto-graph;
    // the workspace root is two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("callisto-graph must be nested two levels under the workspace root")
        .to_path_buf()
}

/// Strips every `#[cfg(test)] mod ... { ... }` block from `content` via
/// brace-depth tracking, rather than truncating at the first occurrence --
/// several files in this workspace interleave multiple `#[cfg(test)]`
/// modules with production code rather than appending one at the very end
/// (e.g. `locate/membership.rs`).
pub fn strip_test_modules(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut out = String::with_capacity(content.len());
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "#[cfg(test)]" {
            i += 1;
            let mut depth: i32 = 0;
            let mut started = false;
            while i < lines.len() {
                for ch in lines[i].chars() {
                    match ch {
                        '{' => {
                            depth += 1;
                            started = true;
                        }
                        '}' => depth -= 1,
                        _ => {}
                    }
                }
                // A brace-less item (`mod tests;`, a test-only `use`) ends at its semicolon.
                let declaration_ended = !started && lines[i].trim_end().ends_with(';');
                i += 1;
                if declaration_ended || (started && depth <= 0) {
                    break;
                }
            }
            continue;
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

pub fn collect_rust_src_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_src_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
            && path.file_name().and_then(|n| n.to_str()) != Some("tests.rs")
        {
            out.push(path);
        }
    }
}
