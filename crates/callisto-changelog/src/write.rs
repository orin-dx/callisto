use std::fs;
use std::path::Path;

use callisto_model::Version;

use crate::ChangelogError;

pub fn prepend(
    root: &Path,
    changelog_path: &Path,
    display_name: &str,
    rendered: &str,
) -> Result<(), ChangelogError> {
    let full_path = root.join(changelog_path);
    if let Some(parent) = full_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let existing = if full_path.exists() {
        fs::read_to_string(&full_path).map_err(|e| ChangelogError::ReadFailed {
            path: changelog_path.to_path_buf(),
            message: e.to_string(),
        })?
    } else {
        format!("# {display_name}\n\n")
    };

    let mut new_content = String::new();
    if let Some(rest) = existing.strip_prefix(&format!("# {display_name}\n\n")) {
        new_content.push_str(&format!("# {display_name}\n\n"));
        new_content.push_str(rendered);
        if !rendered.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push('\n');
        new_content.push_str(rest);
    } else if let Some(rest) = existing.strip_prefix(&format!("# {display_name}\n")) {
        new_content.push_str(&format!("# {display_name}\n\n"));
        new_content.push_str(rendered);
        if !rendered.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push('\n');
        new_content.push_str(rest);
    } else {
        new_content.push_str(rendered);
        if !rendered.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push('\n');
        new_content.push_str(&existing);
    }

    fs::write(&full_path, new_content).map_err(|e| ChangelogError::WriteFailed {
        path: changelog_path.to_path_buf(),
        message: e.to_string(),
    })
}

pub fn extract_section<'a>(changelog: &'a str, version: &Version) -> Option<&'a str> {
    let target_heading = format!("## {}", version.render());
    let mut start_byte = None;
    let mut current_offset = 0;

    for line in changelog.lines() {
        let line_len = line.len() + 1; // line + newline
        if line.trim_end() == target_heading {
            start_byte = Some(current_offset + line_len);
            break;
        }
        current_offset += line_len;
    }

    let start = start_byte?;
    let slice_after = &changelog[start.min(changelog.len())..];

    let mut end = slice_after.len();
    let mut line_offset = 0;

    for line in slice_after.lines() {
        let trimmed = line.trim_start();
        if line_offset > 0 && (trimmed.starts_with("## ") || trimmed.starts_with("# ")) {
            end = line_offset;
            break;
        }
        line_offset += line.len() + 1;
    }

    let raw_section = slice_after[..end.min(slice_after.len())].trim();
    if raw_section.is_empty() {
        None
    } else {
        Some(raw_section)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::VersionGrammar;

    #[test]
    fn extracts_rendered_section() {
        let changelog = r#"# my-pkg

## 1.1.0

### Minor Changes

- Add cool feature

## 1.0.0

### Patch Changes

- Initial release
"#;
        let v1_1 = Version::parse("1.1.0", VersionGrammar::SemVer).unwrap();
        let extracted = extract_section(changelog, &v1_1).unwrap();
        assert!(extracted.contains("### Minor Changes"));
        assert!(extracted.contains("- Add cool feature"));
        assert!(!extracted.contains("## 1.0.0"));
    }
}
