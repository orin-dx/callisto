use std::io::{self, Write};

use serde::Serialize;

use crate::cli::OutputFormat;

pub fn write_json<W: io::Write, S: Serialize + ?Sized>(w: &mut W, val: &S) -> io::Result<()> {
    let text = serde_json::to_string_pretty(val)?;
    writeln!(w, "{text}")
}

/// Serializes any `Report` value to the one JSON success envelope every
/// command shares: `{schemaVersion, command, dryRun, ...payload,
/// diagnostics}`. `command` comes from `R::COMMAND`, `schemaVersion` from
/// the payload's own `Report::schema_version()` (re-inserted at the top
/// level rather than left flattened, so it appears exactly once). Callers
/// no longer hand-build a JSON envelope per command.
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

/// Writes `bytes` to stdout. A closed reading end (e.g. `callisto ... | head`)
/// is not a failure -- it's the normal way a consumer stops reading -- so a
/// `BrokenPipe` is swallowed here instead of propagating, which is what
/// makes this the one sink callers use in place of `println!` (which panics
/// on that same condition).
pub fn write_stdout(bytes: &[u8]) -> io::Result<()> {
    write_swallowing_broken_pipe(&mut io::stdout(), bytes)
}

/// The actual classification [`write_stdout`] applies, pulled out so a test
/// can exercise it against a real closed pipe without touching the
/// process's real (unredirectable, in-process) stdout fd.
fn write_swallowing_broken_pipe<W: Write>(w: &mut W, bytes: &[u8]) -> io::Result<()> {
    match w.write_all(bytes) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(e),
    }
}

/// Writes one line (plus trailing newline) through [`write_stdout`].
pub fn emit_line(line: &str) -> io::Result<()> {
    write_stdout(format!("{line}\n").as_bytes())
}

pub fn log_line(format: OutputFormat, line: &str) -> io::Result<()> {
    match format {
        OutputFormat::Json => {
            eprintln!("{line}");
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
    fn write_json_serializes_pretty_with_trailing_newline() {
        let mut buf = Vec::new();
        write_json(&mut buf, &FakeReport { value: 42 }).unwrap();
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

    /// Against a *real* closed pipe (a `UnixStream` half whose peer was
    /// dropped, not a simulated error), the raw write must actually fail
    /// with `BrokenPipe` (positive proof this test exercises the real
    /// condition, not a vacuously-true no-op), and
    /// `write_swallowing_broken_pipe` -- the classification `write_stdout`
    /// applies to the real stdout fd -- must convert that into `Ok(())`
    /// rather than let it propagate (`println!`'s failure mode).
    #[cfg(unix)]
    #[test]
    fn write_swallowing_broken_pipe_converts_a_real_broken_pipe_to_ok() {
        use std::os::unix::net::UnixStream;

        let (mut writer, reader) = UnixStream::pair().expect("create socket pair");
        drop(reader);

        let raw_err = writer
            .write_all(&[0u8; 4096])
            .expect_err("writing to a closed UnixStream peer must fail");
        assert_eq!(
            raw_err.kind(),
            io::ErrorKind::BrokenPipe,
            "test setup must reproduce a real BrokenPipe, got: {raw_err:?}"
        );

        let result = write_swallowing_broken_pipe(&mut writer, b"more data after the pipe broke");
        assert!(result.is_ok(), "must swallow BrokenPipe, got: {result:?}");
    }

    /// A non-`BrokenPipe` I/O error must still propagate -- this isn't a
    /// blanket "ignore every write error", only the closed-pipe case.
    #[test]
    fn write_swallowing_broken_pipe_propagates_other_errors() {
        struct AlwaysFails;
        impl Write for AlwaysFails {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "nope"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let result = write_swallowing_broken_pipe(&mut AlwaysFails, b"data");
        assert_eq!(
            result.unwrap_err().kind(),
            io::ErrorKind::PermissionDenied,
            "a non-BrokenPipe error must still propagate"
        );
    }

    /// No source file under `callisto-cli/src` may call the raw `println!`
    /// macro outside a `#[cfg(test)]` module: it panics on a closed stdout
    /// (`BrokenPipe`) instead of exiting quietly, unlike
    /// [`write_stdout`]/[`emit_line`]. `eprintln!` (stderr) is unaffected.
    #[test]
    fn println_macro_is_banned_outside_test_modules() {
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        visit_rs_files(&src_dir, &mut offenders);
        assert!(
            offenders.is_empty(),
            "println! is banned in callisto-cli/src outside #[cfg(test)] modules; use \
             output::write_stdout/emit_line instead. Offending locations:\n{}",
            offenders.join("\n")
        );
    }

    fn visit_rs_files(dir: &std::path::Path, offenders: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("read_dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit_rs_files(&path, offenders);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                check_file_for_bare_println(&path, offenders);
            }
        }
    }

    /// Scans only the portion of the file before its first `#[cfg(test)]`
    /// module (every file in this crate puts its test module last), skips
    /// comment lines (this very file's doc comments mention `println!` by
    /// name), and flags a `println!` call that is not part of `eprintln!`.
    fn check_file_for_bare_println(path: &std::path::Path, offenders: &mut Vec<String>) {
        let text = std::fs::read_to_string(path).expect("read source file");
        let scope = text.split("#[cfg(test)]").next().unwrap_or(&text);
        for (i, line) in scope.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let mut search_from = 0;
            while let Some(pos) = line[search_from..].find("println!") {
                let idx = search_from + pos;
                let preceded_by_e = idx > 0 && line.as_bytes()[idx - 1] == b'e';
                if !preceded_by_e {
                    offenders.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
                }
                search_from = idx + "println!".len();
            }
        }
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
