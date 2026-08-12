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
pub fn write_report_json<W: io::Write, R: callisto_model::Report>(
    w: &mut W,
    val: &R,
) -> io::Result<()> {
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
