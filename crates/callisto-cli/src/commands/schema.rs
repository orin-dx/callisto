use std::process::ExitCode;

use callisto_format::{Changeset, PreState};
use callisto_model::{
    InitReport, MatrixReport, PublishPlan, SnapshotReport, StatusReport, TagReport, ValidateReport, VersionReport,
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
        "tag" => schema_for!(TagReport),
        "init" => schema_for!(InitReport),
        "plan-publish" | "publish-plan" => schema_for!(PublishPlan),
        "changeset" => schema_for!(Changeset),
        "pre" => schema_for!(PreState),
        "matrix" => schema_for!(MatrixReport),
        other => {
            return Err(CliError::Other(format!(
                "Unknown schema target type `{other}`. Supported types: status, version, snapshot, validate, tag, init, plan-publish, changeset, pre, matrix"
            )));
        }
    };

    let json = serde_json::to_string_pretty(&schema)
        .map_err(|e| CliError::Other(format!("Failed to serialize JSON schema: {e}")))?;
    println!("{json}");
    Ok(ExitCode::SUCCESS)
}
