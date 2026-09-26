use std::process::ExitCode;

use callisto_format::{Changeset, PreState};
use callisto_model::{
    InitReport, MatrixReport, ReleaseReceiptV1, SnapshotReport, StatusReport, ValidateReport, VersionReport,
};
use schemars::schema_for;

use crate::cli::{GlobalArgs, SchemaArgs};
use crate::error::CliError;

pub fn handle(args: SchemaArgs, _global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let schema = match args.target_type.as_deref().unwrap_or("status") {
        "status" => schema_for!(StatusReport),
        "version" => schema_for!(VersionReport),
        "snapshot" => schema_for!(SnapshotReport),
        "validate" => schema_for!(ValidateReport),
        "init" => schema_for!(InitReport),
        "changeset" => schema_for!(Changeset),
        "pre" => schema_for!(PreState),
        "matrix" => schema_for!(MatrixReport),
        "release-receipt" => schema_for!(ReleaseReceiptV1),
        other => {
            return Err(CliError::UnknownSchemaType {
                requested: other.to_string(),
            });
        }
    };

    // A `schemars::Schema` is a plain JSON tree of strings, numbers, and nested maps;
    // it always serializes.
    let json = serde_json::to_string_pretty(&schema).expect("schema serializes");
    println!("{json}");
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

    #[test]
    fn handle_rejects_unknown_target_type() {
        let result = handle(
            SchemaArgs {
                target_type: Some("bogus".to_string()),
            },
            &global(),
        );
        match result {
            Err(CliError::UnknownSchemaType { requested }) => {
                assert_eq!(requested, "bogus");
                let msg = CliError::UnknownSchemaType { requested }.to_string();
                assert!(msg.contains("Unknown schema target type `bogus`"), "got: {msg}");
                assert!(msg.contains("Supported types:"), "got: {msg}");
            }
            other => panic!("expected CliError::UnknownSchemaType, got: {other:?}"),
        }
    }
}
