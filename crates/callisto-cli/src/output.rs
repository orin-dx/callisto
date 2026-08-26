use std::io;

use serde::Serialize;

use crate::cli::OutputFormat;

pub fn write_json<W: io::Write, S: Serialize + ?Sized>(w: &mut W, val: &S) -> io::Result<()> {
    let text = serde_json::to_string_pretty(val)?;
    writeln!(w, "{text}")
}

/// Serializes a `Report` value to JSON, injecting a `"command"` discriminator
/// field so consumers can distinguish report types (e.g. `"plan-publish"` vs
/// `"publish"`) without inspecting payload structure.
pub fn write_report_json<W: io::Write, R: callisto_model::Report>(w: &mut W, val: &R) -> io::Result<()> {
    #[derive(Serialize)]
    struct WithCommand<'a, T: Serialize> {
        command: &'static str,
        #[serde(flatten)]
        data: &'a T,
    }
    let tagged = WithCommand {
        command: R::COMMAND,
        data: val,
    };
    let text = serde_json::to_string_pretty(&tagged)?;
    writeln!(w, "{text}")
}

pub fn log_line(format: OutputFormat, line: &str) {
    match format {
        OutputFormat::Json => eprintln!("{line}"),
        OutputFormat::Text => println!("{line}"),
    }
}

/// Trait implemented by CLI report structures to guarantee clean JSON stream splitting and diagnostic card formatting.
pub trait ReportPresenter: Serialize {
    fn present_json<W: io::Write>(&self, w: &mut W) -> io::Result<()> {
        write_json(w, self)
    }

    fn present_human(&self) -> String;
}

#[cfg(test)]
mod tests {
    use callisto_model::Diagnostic;
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

    impl ReportPresenter for FakeReport {
        fn present_human(&self) -> String {
            format!("value={}", self.value)
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
    fn write_report_json_injects_command_discriminator() {
        let mut buf = Vec::new();
        write_report_json(&mut buf, &FakeReport { value: 7 }).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("\"command\": \"fake-report\""), "got:\n{text}");
        assert!(text.contains("\"value\": 7"), "got:\n{text}");
    }

    #[test]
    fn present_json_default_impl_delegates_to_write_json() {
        let mut buf = Vec::new();
        FakeReport { value: 1 }.present_json(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("\"value\": 1"), "got:\n{text}");
    }
}
