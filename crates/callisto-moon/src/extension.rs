use serde::{Deserialize, Serialize};

use callisto_graph::locate::LocateError;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RegisterExtensionInput {
    pub moon_version: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RegisterExtensionOutput {
    pub name: &'static str,
    pub description: &'static str,
    pub version: &'static str,
}

pub fn check_moon_version(version_str: &str) -> Result<(), LocateError> {
    if let Ok(ver) = semver::Version::parse(version_str) {
        let req = semver::VersionReq::parse(">=1.30.0, <2.0.0").unwrap();
        if req.matches(&ver) {
            return Ok(());
        }
    }
    Err(LocateError::IncompatibleMoonVersion {
        found: version_str.to_string(),
        required: ">=1.30.0, <2.0.0".to_string(),
    })
}

pub fn register_extension(
    input: RegisterExtensionInput,
) -> Result<RegisterExtensionOutput, LocateError> {
    check_moon_version(&input.moon_version)?;
    Ok(RegisterExtensionOutput {
        name: "callisto",
        description: "Unified multi-ecosystem release manager for moon workspaces",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DefineExtensionConfigOutput {
    pub schema: serde_json::Value,
}

pub fn define_extension_config() -> DefineExtensionConfigOutput {
    DefineExtensionConfigOutput {
        schema: serde_json::json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {}
        }),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExecuteExtensionInput {
    pub args: Vec<String>,
    pub workspace_root: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExecuteExtensionOutput {
    pub report: serde_json::Value,
    pub rendered: String,
    pub exit_code: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InitializeExtensionInput {
    pub workspace_root: String,
    pub confirmed: bool,
}

pub fn execute_extension(input: ExecuteExtensionInput) -> ExecuteExtensionOutput {
    use callisto_graph::commands::status::{status, StatusOptions};
    use std::path::PathBuf;

    let root = PathBuf::from(&input.workspace_root);
    let runner = crate::runner::MoonCommandRunner;
    let locator = match crate::locator::MoonProjectLocator::new(&runner, root.clone()) {
        Ok(loc) => loc,
        Err(e) => {
            let json_val = serde_json::json!({ "error": e.to_string() });
            return ExecuteExtensionOutput {
                report: json_val.clone(),
                rendered: e.to_string(),
                exit_code: 1,
            };
        }
    };

    let ws = match callisto_graph::Workspace::load(root, &locator, &runner) {
        Ok(ws) => ws,
        Err(e) => {
            let json_val = serde_json::json!({ "error": e.to_string() });
            return ExecuteExtensionOutput {
                report: json_val.clone(),
                rendered: e.to_string(),
                exit_code: 1,
            };
        }
    };

    let opts = StatusOptions {
        strict: false,
        strict_graph: false,
    };

    match status(&ws, &opts) {
        Ok(report) => {
            let json_val = serde_json::to_value(&report).unwrap_or_default();
            let rendered = serde_json::to_string_pretty(&json_val).unwrap_or_default();
            ExecuteExtensionOutput {
                report: json_val,
                rendered,
                exit_code: 0,
            }
        }
        Err(e) => {
            let json_val = serde_json::json!({ "error": e.to_string() });
            ExecuteExtensionOutput {
                report: json_val.clone(),
                rendered: e.to_string(),
                exit_code: 1,
            }
        }
    }
}

pub fn initialize_extension(
    input: InitializeExtensionInput,
) -> Result<callisto_model::InitReport, LocateError> {
    use callisto_graph::commands::init::{init, InitOptions};
    use std::path::PathBuf;

    let root = PathBuf::from(&input.workspace_root);
    let runner = crate::runner::MoonCommandRunner;
    let locator = crate::locator::MoonProjectLocator::new(&runner, root.clone())?;
    let ws = callisto_graph::Workspace::load(root, &locator, &runner)
        .map_err(|e| LocateError::Graph(Box::new(e)))?;
    let opts = InitOptions {
        yes: input.confirmed,
    };

    init(&ws, &opts).map_err(|e| LocateError::Graph(Box::new(e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_moon_version_check() {
        assert!(check_moon_version("1.30.0").is_ok());
        assert!(check_moon_version("1.45.2").is_ok());
        assert!(check_moon_version("1.29.9").is_err());
        assert!(check_moon_version("2.0.0").is_err());
    }
}
