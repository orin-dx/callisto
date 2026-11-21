//! The Git boundary: pathspecs go in only as [`Pathspec`], and a command's stdout comes out only through
//! [`GitStdout`], read in the framing that command prints.

use std::path::{Path, PathBuf};

use super::VcsError;
use crate::CommandOutput;

/// A pathspec argument. There is no constructor from raw text, so a path cannot be matched as a glob or magic word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pathspec(String);

impl Pathspec {
    /// `path` matched literally, so a directory named with `*` or `?` selects only itself.
    pub fn literal(path: &Path) -> Self {
        Self(format!(":(literal){}", path.to_string_lossy()))
    }

    /// Everything except `path`, matched literally.
    pub fn excluded(path: &Path) -> Self {
        Self(format!(":(exclude,literal){}", path.to_string_lossy()))
    }

    /// Every file named `file_name`, at any depth; the name itself is matched literally.
    pub fn named_anywhere(file_name: &str) -> Self {
        let mut spec = String::from(":(glob)**/");
        for c in file_name.chars() {
            if matches!(c, '*' | '?' | '[' | '\\') {
                spec.push('\\');
            }
            spec.push(c);
        }
        Self(spec)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a Git command that exited 0 printed.
#[derive(Debug)]
pub struct GitStdout(String);

impl GitStdout {
    /// The stdout of a successful command; a failed one is handed back for the caller's own error.
    pub fn of(output: CommandOutput) -> Result<Self, CommandOutput> {
        if output.success() {
            Ok(Self(output.stdout))
        } else {
            Err(output)
        }
    }

    /// A one-line answer, such as an object id or a ref, without its line terminator.
    pub fn line(&self) -> &str {
        self.0.strip_suffix('\n').unwrap_or(&self.0)
    }

    /// One item per line, for items that cannot hold a newline, such as object ids and ref names.
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.0.lines()
    }

    /// A path printed on its own line: its own text, whitespace included, with only the line terminator removed.
    pub fn path(&self) -> PathBuf {
        PathBuf::from(self.line())
    }

    /// Everything printed, such as a file's contents from `git show <rev>:<path>`.
    pub fn into_text(self) -> String {
        self.0
    }

    /// `-z` output: each NUL-terminated field, never quoted.
    pub fn fields(&self) -> impl Iterator<Item = &str> {
        let fields = self.0.strip_suffix('\0').unwrap_or(&self.0);
        (!fields.is_empty()).then(|| fields.split('\0')).into_iter().flatten()
    }

    /// `git log -z --name-only` run with [`LogEntry::format`]: each commit's header fields and the files it lists.
    pub fn log_entries(&self) -> Result<Vec<LogEntry<'_>>, VcsError> {
        let malformed = |detail: String| VcsError::MalformedLogOutput { detail };
        let mut fields = self.fields().peekable();
        let mut entries = Vec::new();
        while let Some(opener) = fields.next() {
            if !opener.is_empty() {
                return Err(malformed(format!("expected a commit to open with NUL, got {opener:?}")));
            }
            let header = fields
                .next()
                .ok_or_else(|| malformed("expected a header field after the opening NUL".to_owned()))?;
            let mut files = Vec::new();
            while let Some(file) = fields.next_if(|file| !file.is_empty()) {
                // Git puts a newline between the header and the first file.
                files.push(if files.is_empty() {
                    file.strip_prefix('\n').unwrap_or(file)
                } else {
                    file
                });
            }
            entries.push(LogEntry { header, files });
        }
        Ok(entries)
    }
}

/// One commit of [`GitStdout::log_entries`].
#[derive(Debug, PartialEq, Eq)]
pub struct LogEntry<'a> {
    pub header: &'a str,
    pub files: Vec<&'a str>,
}

impl LogEntry<'_> {
    /// The `--format` argument whose output [`GitStdout::log_entries`] reads: each commit opens with NUL, which no
    /// header field or file name can hold, followed by the `placeholder`.
    pub fn format(placeholder: &str) -> String {
        format!("--format=%x00{placeholder}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdout(text: &str) -> GitStdout {
        GitStdout(text.to_owned())
    }

    #[test]
    fn a_failed_command_has_no_stdout_to_read() {
        let failed = CommandOutput {
            exit_code: Some(1),
            stdout: "x\n".to_owned(),
            stderr: String::new(),
        };
        assert_eq!(GitStdout::of(failed.clone()).unwrap_err(), failed);
    }

    #[test]
    fn a_path_keeps_its_own_whitespace() {
        assert_eq!(stdout(" sub dir /\n").path(), PathBuf::from(" sub dir /"));
        assert_eq!(stdout("abc\n\n").line(), "abc\n");
    }

    #[test]
    fn nul_fields_are_split_on_their_terminator_only() {
        assert_eq!(stdout("a b\0\nc\0").fields().collect::<Vec<_>>(), ["a b", "\nc"]);
        assert_eq!(stdout("").fields().count(), 0);
    }

    #[test]
    fn log_entries_group_files_under_their_commit() {
        let log = stdout("\x001\0\n\nodd\0b\0\x002\0\nc\0\x003\0");
        assert_eq!(
            log.log_entries().unwrap(),
            [
                LogEntry {
                    header: "1",
                    files: vec!["\nodd", "b"],
                },
                LogEntry {
                    header: "2",
                    files: vec!["c"],
                },
                LogEntry {
                    header: "3",
                    files: vec![],
                },
            ]
        );
        assert!(stdout("1\0s\0").log_entries().is_err());
    }
}
