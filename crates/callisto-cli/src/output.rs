use std::io;

use serde::Serialize;

use crate::cli::OutputFormat;

pub fn write_json<W: io::Write, S: Serialize>(w: &mut W, val: &S) -> io::Result<()> {
    let text = serde_json::to_string_pretty(val)?;
    writeln!(w, "{text}")
}

pub fn log_line(format: OutputFormat, line: &str) {
    match format {
        OutputFormat::Json => eprintln!("{line}"),
        OutputFormat::Text => println!("{line}"),
    }
}
