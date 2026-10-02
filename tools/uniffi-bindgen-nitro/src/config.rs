//! The `[bindings.nitro]` section of a crate's `uniffi.toml`.

use crate::error::{Error, Result};
use crate::naming::pascal;

/// Settings a crate can supply for the Nitro target.
#[derive(Debug, Clone)]
pub struct NitroConfig {
    /// Nitro module name, also the name of the root hybrid object.
    pub module_name: String,
    /// C++ namespace nested under `margelo::nitro`.
    pub cxx_namespace: String,
    /// Name of the cdylib holding the UniFFI symbols.
    pub cdylib_name: String,
    /// Exported functions to expose as `Promise`-returning methods.
    ///
    /// The Rust function stays synchronous; the generated adapter runs it on a
    /// background thread so a long call cannot block the JavaScript thread.
    pub async_methods: Vec<String>,
}

impl NitroConfig {
    /// Read the config from a crate's `uniffi.toml`, falling back to defaults
    /// derived from the crate name.
    pub fn load(toml_text: Option<&str>, crate_name: &str, path: &str) -> Result<Self> {
        let default_module = pascal(crate_name);
        let mut config = Self {
            module_name: default_module.clone(),
            cxx_namespace: default_module.to_lowercase(),
            cdylib_name: crate_name.replace('-', "_"),
            async_methods: Vec::new(),
        };

        let Some(text) = toml_text else {
            return Ok(config);
        };
        let value: toml::Value =
            toml::from_str(text).map_err(|err: toml::de::Error| Error::Config {
                path: path.to_string(),
                reason: err.to_string(),
            })?;
        let Some(section) = value.get("bindings").and_then(|b| b.get("nitro")) else {
            return Ok(config);
        };

        if let Some(name) = section.get("module_name").and_then(toml::Value::as_str) {
            config.module_name = name.to_string();
            config.cxx_namespace = name.to_lowercase();
        }
        if let Some(namespace) = section.get("cxx_namespace").and_then(toml::Value::as_str) {
            config.cxx_namespace = namespace.to_string();
        }
        if let Some(cdylib) = section.get("cdylib_name").and_then(toml::Value::as_str) {
            config.cdylib_name = cdylib.to_string();
        }
        if let Some(methods) = section.get("async_methods").and_then(toml::Value::as_array) {
            config.async_methods = methods
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect();
        }

        Ok(config)
    }

    /// Whether a Rust function name was marked for off-thread execution.
    pub fn is_async(&self, rust_name: &str) -> bool {
        self.async_methods.iter().any(|name| name == rust_name)
    }
}
