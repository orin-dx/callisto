use std::io;

use serde::Serialize;

use crate::cli::OutputFormat;

pub fn write_json<W: io::Write, S: Serialize + ?Sized>(w: &mut W, val: &S) -> io::Result<()> {
    let text = serde_json::to_string_pretty(val)?;
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
