use std::process::ExitCode;

use callisto_format::{Changeset, PreState};
use callisto_model::{InitReport, MatrixReport, ReleaseReceiptV1, SnapshotReport, StatusReport, VersionReport};
use schemars::schema_for;

use crate::cli::{GlobalArgs, SchemaArgs, SchemaReportType};
use crate::error::CliError;
use crate::output::write_stdout;

pub fn handle(args: SchemaArgs, _global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let schema = match args.target_type.unwrap_or_default() {
        SchemaReportType::Status => schema_for!(StatusReport),
        SchemaReportType::Version => schema_for!(VersionReport),
        SchemaReportType::Snapshot => schema_for!(SnapshotReport),
        SchemaReportType::Init => schema_for!(InitReport),
        SchemaReportType::Changeset => schema_for!(Changeset),
        SchemaReportType::PreState => schema_for!(PreState),
        SchemaReportType::Matrix => schema_for!(MatrixReport),
        SchemaReportType::ReleaseReceipt => schema_for!(ReleaseReceiptV1),
    };

    // A `schemars::Schema` is a plain JSON tree of strings, numbers, and nested maps;
    // it always serializes.
    let json = serde_json::to_string_pretty(&schema).expect("schema serializes");
    write_stdout(format!("{json}\n").as_bytes())?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn global() -> GlobalArgs {
        GlobalArgs {
            format: crate::cli::OutputFormat::Text,
            cwd: std::path::PathBuf::from("."),
            dry_run: false,
        }
    }

    /// Every registry entry actually produces a schema; an unknown `--type`
    /// value is rejected earlier, at clap parse time (see `cli.rs`'s
    /// `schema_type_rejects_unknown_value_as_a_usage_error`), so this
    /// handler-level test only needs to cover the exhaustive match's arms.
    #[test]
    fn handle_succeeds_for_every_registered_type() {
        for target_type in [
            SchemaReportType::Status,
            SchemaReportType::Version,
            SchemaReportType::Snapshot,
            SchemaReportType::Init,
            SchemaReportType::Changeset,
            SchemaReportType::PreState,
            SchemaReportType::Matrix,
            SchemaReportType::ReleaseReceipt,
        ] {
            let result = handle(
                SchemaArgs {
                    target_type: Some(target_type),
                },
                &global(),
            );
            assert!(result.is_ok(), "target_type={target_type:?}");
        }
    }
}
