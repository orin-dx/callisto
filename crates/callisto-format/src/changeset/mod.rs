mod frontmatter;
#[cfg(test)]
mod tests;

use callisto_model::{Severity, SeverityParseError};
use frontmatter::{needs_quoting, parse_entry_line, LineError};

/// One parsed `.changeset/*.md` file (§6.1's shape): a frontmatter block of
/// `name: severity` entries, followed by a free-text summary.
///
/// Deliberately does not carry a filename or path — this crate is filesystem-free; a caller
/// that reads files off disk attaches the filename itself for sort ordering and error
/// context.
use schemars::JsonSchema;

/// One parsed `.changeset/*.md` file (§6.1's shape): a frontmatter block of
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

impl Changeset {
    /// Convenience wrapper around [`write_changeset`].
    pub fn to_markdown(&self) -> Result<String, WriteError> {
        write_changeset(self)
    }
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

    /// §6.1: "Empty frontmatter valid iff summary is non-empty."
    #[error("changeset has no frontmatter entries and an empty summary")]
    #[diagnostic(code(E048))]
    EmptyChangeset,

    /// A changeset with one or more entries must have a non-empty summary.
    #[error("changeset has entries but an empty or whitespace-only summary")]
    #[diagnostic(code(E055), help("Add a non-empty summary after the closing `---` delimiter."))]
    EmptySummary,
}

#[derive(Debug, thiserror::Error, miette::Diagnostic, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteError {
    /// Mirrors `ParseError::EmptyChangeset`.
    #[error("cannot write changeset: no entries and an empty summary")]
    #[diagnostic(code(E049))]
    EmptyChangeset,

    /// A changeset with one or more entries must have a non-empty summary.
    #[error("cannot write changeset: entries present but summary is empty or whitespace-only")]
    #[diagnostic(code(E056), help("Provide a non-empty summary describing the change."))]
    EmptySummary,

    /// `entries[index]`'s name is the empty string.
    #[error("entry {index} has an empty package name")]
    #[diagnostic(code(E057))]
    EmptyName { index: usize },

    /// `entries[index]`'s name contains a literal `"`, which cannot be written — no escaping
    /// convention is defined for this grammar.
    #[error("entry {index} name {name:?} contains a literal `\"`, which cannot be written (no escaping convention is defined for this grammar)")]
    NameContainsQuote { index: usize, name: String },
}

/// Parses one `.changeset/*.md` file's contents.
///
/// Grammar (§6.1): a `---`-delimited frontmatter block starting on line 1, each non-blank,
/// non-comment line inside it shaped `<name>: <severity>`, followed by the file's remaining
/// content as `summary` (trimmed). `#`-comment lines and blank lines inside the frontmatter
/// block are skipped. CRLF line endings are normalized to LF before parsing.
pub fn parse_changeset(source: &str) -> Result<Changeset, ParseError> {
    let trimmed_bom = source.strip_prefix('\u{FEFF}').unwrap_or(source);
    let normalized = trimmed_bom.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();

    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return Err(ParseError::MissingFrontmatterStart);
    }

    // Two-pass, deliberately: find the frontmatter's boundaries FIRST, then parse content
    // within them. A single pass that tries to parse every line as an entry until it
    // happens to hit a literal "---" cannot tell "this entry is malformed" apart from "the
    // frontmatter was never closed at all" — the first non-entry-shaped line after a missing
    // closing delimiter would otherwise surface as a misleading parse error on that line
    // instead of `UnclosedFrontmatter`.
    let closing_index = lines[1..].iter().position(|&l| l.trim_end() == "---").map(|i| i + 1);
    let Some(closing_index) = closing_index else {
        return Err(ParseError::UnclosedFrontmatter);
    };

    let mut entries: Vec<Entry> = Vec::new();
    let mut first_seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for (offset, &line) in lines[1..closing_index].iter().enumerate() {
        let line_no = offset + 2; // absolute, 1-indexed; line 1 was "---"
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let entry = parse_entry_line(line).map_err(|e| promote_line_error(e, line_no))?;
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

    if entries.is_empty() && summary.is_empty() {
        return Err(ParseError::EmptyChangeset);
    }
    if !entries.is_empty() && summary.is_empty() {
        return Err(ParseError::EmptySummary);
    }

    Ok(Changeset { entries, summary })
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
pub fn write_changeset(changeset: &Changeset) -> Result<String, WriteError> {
    if changeset.summary.trim().is_empty() {
        if changeset.entries.is_empty() {
            return Err(WriteError::EmptyChangeset);
        }
        return Err(WriteError::EmptySummary);
    }
    for (index, entry) in changeset.entries.iter().enumerate() {
        if entry.name.is_empty() {
            return Err(WriteError::EmptyName { index });
        }
        if entry.name.contains('"') {
            return Err(WriteError::NameContainsQuote {
                index,
                name: entry.name.clone(),
            });
        }
    }

    let mut out = String::from("---\n");
    for entry in &changeset.entries {
        if needs_quoting(&entry.name) {
            out.push_str(&format!("\"{}\": {}\n", entry.name, entry.severity));
        } else {
            out.push_str(&format!("{}: {}\n", entry.name, entry.severity));
        }
    }
    out.push_str("---\n\n");
    out.push_str(changeset.summary.trim());
    out.push('\n');
    Ok(out)
}
