//! `execute_extension`/`initialize_extension` -- the two `pdk`-feature
//! functions that transitively call `crate::runner::MoonCommandRunner`'s
//! wasm-only `CommandRunner` impl (via `Workspace::load`/`MoonProjectLocator`),
//! and so can never actually execute inside a native `cargo-llvm-cov`
//! instrumented build (see `runner_pdk.rs`'s doc comment for the underlying
//! wasm-only-extern reason). Split into their own file so
//! `--ignore-filename-regex` can exclude it from coverage reports.
//!
//! Wire-level coverage (including their error paths) lives in
//! `tests/moon_wasm_sandbox.rs`, which drives the real compiled
//! `callisto-moon.wasm` module through an actual Extism/wasmtime host.
//!
//! `register_extension`, `define_extension_config`, `resolve_subcommand`,
//! and `format_graph_error_json` stay in `extension.rs`: none of them call
//! `MoonCommandRunner`, so they link and run fine natively under
//! `--features pdk` and are exercised by real native unit tests there.

use crate::extension::{
    build_extension_output, error_output, format_graph_error_json, resolve_subcommand, ExecuteExtensionOutput,
};
use callisto_graph::locate::LocateError;

#[cfg(feature = "pdk")]
pub fn execute_extension(input: moon_pdk_api::ExecuteExtensionInput) -> ExecuteExtensionOutput {
    use callisto_graph::commands::status::{status, StatusOptions};

    // moon's real `ExecuteExtensionInput` nests the workspace root (and cwd)
    // inside `context: MoonContext` rather than as a flat `workspace_root`
    // field. `MoonContext::workspace_root` is a `VirtualPath`; `.to_path_buf()`
    // returns the (possibly WASI-virtualized) path, matching how the old flat
    // field was consumed below.
    let root = input.context.workspace_root.to_path_buf();
    let runner = crate::runner::MoonCommandRunner;
    let locator = match crate::locator::MoonProjectLocator::new(&runner, root.clone()) {
        Ok(loc) => loc,
        Err(e) => {
            let json_val = serde_json::json!({
                "schemaVersion": callisto_model::SCHEMA_VERSION,
                "error": { "code": "E_LOCATE", "message": e.to_string() }
            });
            return error_output(json_val, &e);
        }
    };

    let release_root = root.clone();
    let ws = match callisto_graph::Workspace::load(root, &locator, &runner) {
        Ok(ws) => ws,
        Err(e) => {
            let json_val = format_graph_error_json(&e);
            return error_output(json_val, &e);
        }
    };

    let subcmd = resolve_subcommand(&input.args);

    if let Some(replacement) = crate::extension::removed_subcommand(subcmd) {
        let message = format!("`{subcmd}` was removed; use `{replacement}`");
        let json_val = serde_json::json!({
            "schemaVersion": callisto_model::SCHEMA_VERSION,
            "error": { "code": "E_REMOVED_SUBCOMMAND", "message": message }
        });
        return error_output(json_val, &message);
    }

    match subcmd {
        "release" => {
            use callisto_graph::commands::{plan_local_release, LocalReleaseSource};
            match plan_local_release(&release_root, &locator, &runner, &[], LocalReleaseSource::Preview) {
                Ok(Some(plan)) => build_extension_output(serde_json::to_value(&plan.intent), 0),
                Ok(None) => build_extension_output(Ok(serde_json::json!({ "nothingToRelease": true })), 0),
                Err(e) => {
                    let json_val = format_graph_error_json(&e);
                    error_output(json_val, &e)
                }
            }
        }
        "validate" => {
            use callisto_graph::commands::validate::{validate, ValidateOptions};
            match validate(&ws, &ValidateOptions::default()) {
                Ok(report) => {
                    let exit_code = if report.ok { 0 } else { 1 };
                    build_extension_output(serde_json::to_value(&report), exit_code)
                }
                Err(e) => {
                    let json_val = format_graph_error_json(&e);
                    error_output(json_val, &e)
                }
            }
        }
        _ => {
            let opts = StatusOptions {
                strict: false,
                strict_graph: false,
            };

            match status(&ws, &opts) {
                Ok(report) => {
                    let has_errors = report
                        .diagnostics
                        .iter()
                        .any(|d| d.severity == callisto_model::DiagnosticSeverity::Error);
                    let exit_code = if has_errors { 1 } else { 0 };
                    build_extension_output(serde_json::to_value(&report), exit_code)
                }
                Err(e) => {
                    let json_val = format_graph_error_json(&e);
                    error_output(json_val, &e)
                }
            }
        }
    }
}

#[cfg(feature = "pdk")]
pub fn initialize_extension(
    input: moon_pdk_api::InitializeExtensionInput,
) -> Result<moon_pdk_api::InitializeExtensionOutput, LocateError> {
    use callisto_graph::commands::init;

    // moon offers no prompts, so this scaffolds the answer-free config: independent versioning, no binaries.
    let root = input.context.workspace_root.to_path_buf();
    // Always a real write: this host surface has no --dry-run equivalent.
    let permit = callisto_model::ApplyPermit::granted_unless_dry_run(false).expect("non-dry-run permits writes");
    let graph_err = |e| LocateError::Graph(Box::new(e));
    let runner = crate::runner::MoonCommandRunner;
    let locator = crate::locator::MoonProjectLocator::new(&runner, root.clone())?;
    callisto_graph::Workspace::load(root.clone(), &locator, &runner).map_err(graph_err)?;
    if !root.join("callisto.toml").exists() {
        init::write(&root, init::empty_config(), &permit).map_err(graph_err)?;
    }
    init::write_changeset_readme(&root, &permit).map_err(graph_err)?;

    Ok(moon_pdk_api::InitializeExtensionOutput {
        config_url: None,
        default_settings: Default::default(),
        docs_url: None,
        prompts: Vec::new(),
    })
}
