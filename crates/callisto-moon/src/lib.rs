//! moon WASM extension for callisto.

pub mod extension;
pub mod locator;
pub mod runner;

pub use extension::*;
pub use locator::*;
pub use runner::*;

#[cfg(feature = "pdk")]
pub mod plugin {
    use super::*;
    use extism_pdk::*;

    #[plugin_fn]
    pub fn register_extension(Json(input): Json<RegisterExtensionInput>) -> FnResult<Json<RegisterExtensionOutput>> {
        let output = extension::register_extension(input);
        Ok(Json(output))
    }

    #[plugin_fn]
    pub fn define_extension_config(_input: ()) -> FnResult<Json<DefineExtensionConfigOutput>> {
        let output = extension::define_extension_config();
        Ok(Json(output))
    }

    #[plugin_fn]
    pub fn execute_extension(Json(input): Json<ExecuteExtensionInput>) -> FnResult<Json<ExecuteExtensionOutput>> {
        let output = extension::execute_extension(input);
        Ok(Json(output))
    }

    #[plugin_fn]
    pub fn initialize_extension(
        Json(input): Json<InitializeExtensionInput>,
    ) -> FnResult<Json<InitializeExtensionOutput>> {
        let output = extension::initialize_extension(input)
            .map_err(|e| WithReturnCode::new(extism_pdk::Error::msg(e.to_string()), 1))?;
        Ok(Json(output))
    }
}
