pub mod add;
pub mod completions;
pub mod compose_pr_body;
pub mod filter_plan;
pub mod init;
pub mod matrix;
pub mod plan_publish;
pub mod pre;
pub mod publish;
pub mod release;
pub mod release_pr;
pub mod schema;
pub mod snapshot;
pub mod status;
pub mod tag;
pub mod validate;
pub mod version;

/// Reads `arg` as a JSON document: a literal `-` reads from stdin, a value
/// starting with `{` is treated as inline JSON, anything else is read as a
/// file path. Shared by every command that accepts a report/plan as either
/// a file, inline JSON, or piped stdin (`tag --plan`, `filter-plan --plan`,
/// `filter-plan --report`).
pub(crate) fn read_json_arg(arg: &str) -> Result<String, crate::error::CliError> {
    if arg == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
        Ok(buf)
    } else if arg.trim_start().starts_with('{') {
        Ok(arg.to_string())
    } else {
        std::fs::read_to_string(arg).map_err(|source| crate::error::CliError::Io {
            source,
            path: Some(std::path::PathBuf::from(arg)),
        })
    }
}
