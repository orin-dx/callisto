use std::io::{self, Write};

use serde::Serialize;

use crate::cli::OutputFormat;

/// Writes a JSON document that is not a report, such as the intent `release inspect` echoes, which carries its own
/// version. A report goes through [`emit_report`], the only way to get the envelope and `schemaVersion`; this is
/// crate-private so a command cannot bypass it.
pub(crate) fn write_document<W: io::Write, S: Serialize + ?Sized>(w: &mut W, val: &S) -> io::Result<()> {
    let text = serde_json::to_string_pretty(val)?;
    writeln!(w, "{text}")
}

/// Serializes any `Report` value to the one JSON success envelope every
/// command shares: `{schemaVersion, command, dryRun, ...payload,
/// diagnostics}`. `command` comes from `R::COMMAND`, `schemaVersion` from
/// the payload's own `Report::schema_version()` (re-inserted at the top
/// level rather than left flattened, so it appears exactly once). Callers
/// pass their `Report` to this function instead of hand-building a JSON envelope.
pub fn emit_report<W: io::Write, R: callisto_model::Report>(w: &mut W, val: &R, dry_run: bool) -> io::Result<()> {
    let mut value = serde_json::to_value(val)?;
    if let serde_json::Value::Object(map) = &mut value {
        map.insert("schemaVersion".to_string(), serde_json::json!(val.schema_version()));
        map.insert("command".to_string(), serde_json::json!(R::COMMAND));
        map.insert("dryRun".to_string(), serde_json::json!(dry_run));
    }
    let text = serde_json::to_string_pretty(&value)?;
    writeln!(w, "{text}")
}

/// Standard output as every command writes it. A reader that closes it early (`callisto status | head`) ends the
/// output, not the command: a write to the closed pipe counts as done. Any other write error is returned.
pub struct Stdout<W>(W);

impl<W: Write> Stdout<W> {
    pub fn new(inner: W) -> Self {
        Self(inner)
    }
}

/// The process's stdout, through [`Stdout`].
#[expect(clippy::disallowed_methods, reason = "the one place stdout is opened for writing")]
pub fn stdout() -> Stdout<io::Stdout> {
    Stdout::new(io::stdout())
}

impl<W: Write> Write for Stdout<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.0.write(buf) {
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(buf.len()),
            result => result,
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.0.flush() {
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            result => result,
        }
    }
}

/// Writes `bytes` to [`stdout`].
pub fn write_stdout(bytes: &[u8]) -> io::Result<()> {
    stdout().write_all(bytes)
}

/// Writes one line (plus trailing newline) through [`write_stdout`].
pub fn emit_line(line: &str) -> io::Result<()> {
    write_stdout(format!("{line}\n").as_bytes())
}

pub use callisto_graph::note::advise;

/// Writes one line of wizard narration (`callisto add`'s step prompts) to stderr, off stdout so a script piping
/// stdout never sees prompt text mixed into a report or changeset preview.
pub fn prompt_line(line: &str) -> io::Result<()> {
    writeln!(io::stderr(), "{line}")
}

/// A progress line: stdout in text; in JSON, where stdout carries only the report, an advisory line on stderr.
pub fn log_line(format: OutputFormat, line: &str) -> io::Result<()> {
    match format {
        OutputFormat::Json => {
            advise(&mut io::stderr(), format_args!("{line}\n"));
            Ok(())
        }
        OutputFormat::Text => emit_line(line),
    }
}

#[cfg(test)]
mod tests {
    use callisto_model::{Diagnostic, Report};
    use serde::Deserialize;

    use super::*;

    #[derive(Serialize, Deserialize)]
    struct FakeReport {
        value: u32,
    }

    impl callisto_model::Report for FakeReport {
        const COMMAND: &'static str = "fake-report";

        fn schema_version(&self) -> u32 {
            1
        }

        fn diagnostics(&self) -> &[Diagnostic] {
            &[]
        }
    }

    #[test]
    fn write_document_serializes_pretty_with_trailing_newline() {
        let mut buf = Vec::new();
        write_document(&mut buf, &FakeReport { value: 42 }).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("\"value\": 42"), "got:\n{text}");
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn emit_report_injects_command_schema_version_and_dry_run() {
        let mut buf = Vec::new();
        emit_report(&mut buf, &FakeReport { value: 7 }, false).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("\"command\": \"fake-report\""), "got:\n{text}");
        assert!(text.contains("\"schemaVersion\": 1"), "got:\n{text}");
        assert!(text.contains("\"dryRun\": false"), "got:\n{text}");
        assert!(text.contains("\"value\": 7"), "got:\n{text}");
    }

    /// The payload's own `schemaVersion` field must not survive alongside the
    /// envelope's -- exactly one `"schemaVersion"` key, not two.
    #[test]
    fn emit_report_does_not_duplicate_schema_version_key() {
        let mut buf = Vec::new();
        emit_report(&mut buf, &FakeReport { value: 1 }, true).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert_eq!(text.matches("\"schemaVersion\"").count(), 1, "got:\n{text}");
        assert!(text.contains("\"dryRun\": true"), "got:\n{text}");
    }

    /// Against a real closed pipe the raw write fails with `BrokenPipe` (so the test reproduces the condition), and
    /// [`Stdout`] counts the write as done.
    #[cfg(unix)]
    #[test]
    fn stdout_counts_a_write_to_a_closed_pipe_as_done() {
        use std::os::unix::net::UnixStream;

        let (mut writer, reader) = UnixStream::pair().expect("create socket pair");
        drop(reader);

        // The peer's close is not always visible to the very first write, so keep writing until it surfaces.
        let raw_err = (0..64)
            .find_map(|_| writer.write_all(&[0u8; 4096]).err())
            .expect("writing to a closed UnixStream peer must fail");
        assert_eq!(raw_err.kind(), io::ErrorKind::BrokenPipe, "{raw_err:?}");

        let mut sink = Stdout::new(writer);
        let result = sink
            .write_all(b"more data after the pipe broke")
            .and_then(|()| sink.flush());
        assert!(result.is_ok(), "{result:?}");
    }

    /// Only the closed pipe is quiet: any other error reaches the caller.
    #[test]
    fn stdout_returns_other_errors() {
        struct AlwaysFails;
        impl Write for AlwaysFails {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "nope"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let result = Stdout::new(AlwaysFails).write_all(b"data");
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn report_trait_accessors_expose_schema_version_and_diagnostics() {
        let report = FakeReport { value: 1 };
        assert_eq!(report.schema_version(), 1);
        assert!(report.diagnostics().is_empty());
    }

    /// `log_line` routes purely by `OutputFormat` (Json -> stderr, Text ->
    /// stdout); it writes directly to the process's real fds rather than a
    /// caller-supplied writer, so its destination isn't capturable in-process.
    /// This exercises both branches to prove neither panics.
    #[test]
    fn log_line_does_not_panic_for_either_output_format() {
        log_line(OutputFormat::Json, "to stderr").unwrap();
        log_line(OutputFormat::Text, "to stdout").unwrap();
    }
}
