mod frontmatter;
#[cfg(test)]
mod tests;

use crate::{Severity, SeverityParseError};
use frontmatter::{needs_quoting, parse_entry_line, LineError};

/// One parsed `.changeset/*.md` file: a frontmatter block of
/// `name: severity` entries, followed by a free-text summary.
///
/// Deliberately does not carry a filename or path — this crate is filesystem-free; a caller
/// that reads files off disk attaches the filename itself for sort ordering and error
/// context.
use schemars::JsonSchema;

/// One parsed `.changeset/*.md` file: a frontmatter block of
/// `name: severity` entries, followed by a free-text summary.
///
/// Deliberately does not carry a filename or path — this crate is filesystem-free; a caller
/// that reads files off disk attaches the filename itself for sort ordering and error
/// context.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct Changeset {
    pub entries: Vec<Entry>,
    pub summary: String,
}

/// One `"name": severity` frontmatter line, after quote resolution.
///
/// `name` is the raw string as written in the changeset — resolving it to a `PackageId`
/// (bare vs. `ecosystem/name`-prefixed) is a workspace-aware operation this crate cannot
/// perform and does not attempt.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct Entry {
    pub name: String,
    pub severity: Severity,
}

#[derive(Debug, thiserror::Error, miette::Diagnostic, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseError {
    /// The file does not open with a `---` delimiter on line 1 at all.
    #[error("changeset does not start with a `---` frontmatter delimiter on line 1")]
    #[diagnostic(
        code(E040),
        help("Add a `---` frontmatter delimiter on line 1 of the changeset file.")
    )]
    MissingFrontmatterStart,

    /// A `---` opened on line 1 but no matching closing `---` line was ever found.
    #[error("frontmatter opened with `---` on line 1 but was never closed with a matching `---`")]
    #[diagnostic(code(E041), help("Ensure frontmatter block closes with `---`."))]
    UnclosedFrontmatter,

    /// A quoted name's opening `"` has no matching closing `"` on the same line.
    #[error("line {line}: quoted name is never closed with a matching `\"`")]
    #[diagnostic(code(E042))]
    UnclosedQuotedName { line: usize },

    /// The closing `"` of a quoted name is not immediately followed by the separator `:`.
    #[error("line {line}: quoted name `{raw}` is followed by unexpected content before the `:` separator")]
    #[diagnostic(code(E043))]
    AmbiguousNameQuoting { line: usize, raw: String },

    /// A bare (unquoted) line contains no `:` at all.
    #[error("line {line}: no `:` separator found in {raw:?}")]
    #[diagnostic(code(E044))]
    MissingSeparator { line: usize, raw: String },

    /// The name resolved to the empty string.
    #[error("line {line}: package name is empty")]
    #[diagnostic(code(E045))]
    EmptyName { line: usize },

    /// The severity token is not one of `major | minor | patch | none` (case-insensitive).
    #[error("line {line}: invalid severity for package {name:?}: {source}")]
    #[diagnostic(code(E046))]
    InvalidSeverity {
        line: usize,
        name: String,
        #[source]
        source: SeverityParseError,
    },

    /// The same (raw, pre-`PackageId`-resolution) name appears twice in one changeset's
    /// frontmatter.
    #[error("line {line}: package {name:?} is named more than once in this changeset's frontmatter (first on line {first_line})")]
    #[diagnostic(code(E047))]
    DuplicateEntry {
        line: usize,
        first_line: usize,
        name: String,
    },

    /// A changeset with one or more entries must have a non-empty summary.
    #[error("changeset has entries but an empty or whitespace-only summary")]
    #[diagnostic(code(E055), help("Add a non-empty summary after the closing `---` delimiter."))]
    EmptySummary,
}

#[derive(Debug, thiserror::Error, miette::Diagnostic, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteError {
    /// A changeset with one or more entries must have a non-empty summary.
    #[error("cannot write changeset: entries present but summary is empty or whitespace-only")]
    #[diagnostic(code(E056), help("Provide a non-empty summary describing the change."))]
    EmptySummary,

    /// `entries[index]`'s name is the empty string.
    #[error("entry {index} has an empty package name")]
    #[diagnostic(code(E057))]
    EmptyName { index: usize },

    /// `entries[index]`'s name would not read back as itself: it holds a `"`, CR or LF (the grammar has no
    /// escaping and is line-based), or repeats an earlier entry's name.
    #[error("entry {index} name {name:?} cannot be written so it reads back unchanged: a changeset name cannot contain `\"`, a carriage return or a line feed, or repeat another entry")]
    #[diagnostic(
        code(E268),
        help("Rename the package so its name has no `\"`, carriage return or line feed, and name each package once.")
    )]
    NameNotRepresentable { index: usize, name: String },
}

/// Parses one `.changeset/*.md` file's contents.
///
/// Grammar: a `---`-delimited frontmatter block starting on line 1, each non-blank,
/// non-comment line inside it shaped `<name>: <severity>`, followed by the file's remaining
/// content as `summary` (trimmed). `#`-comment lines and blank lines inside the frontmatter
/// block are skipped. CRLF line endings are normalized to LF before parsing.
pub fn parse_changeset(source: &str) -> Result<Changeset, ParseError> {
    let trimmed_bom = source.strip_prefix('\u{FEFF}').unwrap_or(source);
    let normalized = lf_line_ends(trimmed_bom);
    let lines: Vec<&str> = normalized.split('\n').collect();

    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return Err(ParseError::MissingFrontmatterStart);
    }

    // Two passes: find the frontmatter boundaries first, so a missing closing delimiter reports
    // `UnclosedFrontmatter` rather than a misleading parse error on the next non-entry line.
    let closing_index = lines[1..].iter().position(|&l| l.trim_end() == "---").map(|i| i + 1);
    let Some(closing_index) = closing_index else {
        return Err(ParseError::UnclosedFrontmatter);
    };

    let mut entries: Vec<Entry> = Vec::new();
    let mut first_seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for (offset, &line) in lines[1..closing_index].iter().enumerate() {
        let line_no = offset + 2; // absolute, 1-indexed; line 1 was "---"
        let Some(entry) = frontmatter_entry(line) else {
            continue;
        };
        let entry = entry.map_err(|e| promote_line_error(e, line_no))?;
        if let Some(&first_line) = first_seen.get(&entry.name) {
            return Err(ParseError::DuplicateEntry {
                line: line_no,
                first_line,
                name: entry.name,
            });
        }
        first_seen.insert(entry.name.clone(), line_no);
        entries.push(entry);
    }

    let summary = lines[closing_index + 1..].join("\n").trim().to_string();

    // No entries and no summary is a valid empty changeset (the shape `@changesets/cli add
    // --empty` writes) -- entries and an empty summary is still rejected below.
    if !entries.is_empty() && summary.is_empty() {
        return Err(ParseError::EmptySummary);
    }

    Ok(Changeset { entries, summary })
}

/// CRLF and lone CR become LF, as the reader splits lines.
fn lf_line_ends(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// One frontmatter line as the reader sees it; `None` for a blank or `#`-comment line.
fn frontmatter_entry(line: &str) -> Option<Result<Entry, LineError>> {
    if line.trim().is_empty() || line.trim_start().starts_with('#') {
        return None;
    }
    Some(parse_entry_line(line))
}

fn promote_line_error(err: LineError, line: usize) -> ParseError {
    match err {
        LineError::UnclosedQuotedName => ParseError::UnclosedQuotedName { line },
        LineError::AmbiguousNameQuoting { raw } => ParseError::AmbiguousNameQuoting { line, raw },
        LineError::MissingSeparator { raw } => ParseError::MissingSeparator { line, raw },
        LineError::EmptyName => ParseError::EmptyName { line },
        LineError::InvalidSeverity { name, source } => ParseError::InvalidSeverity { line, name, source },
    }
}

/// Serializes a [`Changeset`] back to `.changeset/*.md` bytes.
///
/// Names are quoted only when necessary. Severities are always written lowercase. Output
/// always uses `\n` line endings and ends with a single trailing newline after the summary.
/// An empty changeset (no entries, no summary) writes as `---\n---\n\n`.
///
/// Every entry reads back through [`parse_changeset`] with the same name and severity, and the
/// summary reads back as written here (trimmed, LF line ends); a name that cannot is refused.
pub fn write_changeset(changeset: &Changeset) -> Result<String, WriteError> {
    if changeset.summary.trim().is_empty() && !changeset.entries.is_empty() {
        return Err(WriteError::EmptySummary);
    }
    let mut out = String::from("---\n");
    for (index, entry) in changeset.entries.iter().enumerate() {
        if entry.name.is_empty() {
            return Err(WriteError::EmptyName { index });
        }
        let line = if needs_quoting(&entry.name) {
            format!("\"{}\": {}", entry.name, entry.severity)
        } else {
            format!("{}: {}", entry.name, entry.severity)
        };
        // No escapes and one entry per line; the reader rejects a repeated name.
        let reads_back = !entry.name.contains(['"', '\r', '\n'])
            && !changeset.entries[..index]
                .iter()
                .any(|earlier| earlier.name == entry.name)
            && frontmatter_entry(&line).and_then(Result::ok).as_ref() == Some(entry);
        if !reads_back {
            return Err(WriteError::NameNotRepresentable {
                index,
                name: entry.name.clone(),
            });
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("---\n");
    let summary = lf_line_ends(&changeset.summary);
    let summary = summary.trim();
    if !summary.is_empty() {
        out.push('\n');
        out.push_str(summary);
    }
    out.push('\n');
    Ok(out)
}
