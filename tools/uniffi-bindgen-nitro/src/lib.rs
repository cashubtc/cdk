//! Generates React Native Nitro bindings from UniFFI metadata.
//!
//! The UniFFI `#[uniffi::export]` surface is the only interface definition kept
//! by hand: this crate reads the `ComponentInterface` out of a built cdylib and
//! emits the Nitro TypeScript spec, the C++ that crosses the UniFFI ABI, and
//! the Nitro `HybridObject` implementations that sit on top.

pub mod config;
pub mod cpp_bridge;
pub mod cpp_ffi;
pub mod cpp_runtime;
pub mod error;
pub mod hybrid;
pub mod model;
pub mod naming;
pub mod node;
pub mod spec_parser;
pub mod ts;
pub mod typemap;
pub mod writer;

use std::collections::BTreeMap;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use uniffi_bindgen::library_mode::find_components;
use uniffi_bindgen::{BindgenCrateConfigSupplier, ComponentInterface};

pub use crate::config::NitroConfig;
pub use crate::error::{Error, Result};
pub use crate::model::Model;

/// A file the generator wants written.
#[derive(Debug, Clone)]
pub struct GeneratedFile {
    /// Path relative to the output directory it belongs to.
    pub path: Utf8PathBuf,
    /// Complete file contents.
    pub contents: String,
}

impl GeneratedFile {
    /// Write the file, creating parent directories as needed.
    pub fn write(&self, root: &Utf8Path) -> Result<Utf8PathBuf> {
        let target = root.join(&self.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|err| Error::Write {
                path: parent.to_string(),
                reason: err.to_string(),
            })?;
        }
        fs::write(&target, &self.contents).map_err(|err| Error::Write {
            path: target.to_string(),
            reason: err.to_string(),
        })?;
        Ok(target)
    }
}

/// Supplies one `uniffi.toml` for every crate found in the library.
#[derive(Debug, Default)]
struct SingleConfigSupplier {
    table: Option<toml::value::Table>,
}

impl BindgenCrateConfigSupplier for SingleConfigSupplier {
    fn get_toml(&self, _crate_name: &str) -> anyhow::Result<Option<toml::value::Table>> {
        Ok(self.table.clone())
    }
}

/// Read the metadata of one crate out of a built cdylib.
pub fn load(
    library: &Utf8Path,
    crate_name: Option<&str>,
    config_path: Option<&Utf8Path>,
) -> Result<(ComponentInterface, NitroConfig)> {
    let (config_text, config_display) = match config_path {
        Some(path) => {
            let text = fs::read_to_string(path).map_err(|err| Error::Config {
                path: path.to_string(),
                reason: err.to_string(),
            })?;
            (Some(text), path.to_string())
        }
        None => (None, "<none>".to_string()),
    };

    let table = match &config_text {
        Some(text) => {
            Some(
                toml::from_str::<toml::value::Table>(text).map_err(|err| Error::Config {
                    path: config_display.clone(),
                    reason: err.to_string(),
                })?,
            )
        }
        None => None,
    };

    let components = find_components(library, &SingleConfigSupplier { table }).map_err(|err| {
        Error::Metadata {
            library: library.to_string(),
            reason: format!("{err:#}"),
        }
    })?;

    let component = match crate_name {
        Some(wanted) => components
            .into_iter()
            .find(|component| component.ci.crate_name() == wanted)
            .ok_or_else(|| Error::NoSuchComponent {
                crate_name: wanted.to_string(),
            })?,
        None => components
            .into_iter()
            .next()
            .ok_or_else(|| Error::NoSuchComponent {
                crate_name: "<first>".to_string(),
            })?,
    };

    let crate_name = component.ci.crate_name().to_string();
    let config = NitroConfig::load(config_text.as_deref(), &crate_name, &config_display)?;
    Ok((component.ci, config))
}

/// Everything that can be generated before nitrogen runs.
pub fn generate_spec(
    ci: &ComponentInterface,
    config: &NitroConfig,
) -> Result<BTreeMap<&'static str, Vec<GeneratedFile>>> {
    let model = Model::build(ci, config)?;
    let crate_name = ci.crate_name();

    let ffi_header = format!("{}Ffi.hpp", model.module);
    let bridge_header = format!("{}Bridge.hpp", model.module);

    let ts_files = vec![
        GeneratedFile {
            path: Utf8PathBuf::from(format!("{}.nitro.ts", model.module)),
            contents: ts::spec(&model, config, crate_name),
        },
        GeneratedFile {
            path: Utf8PathBuf::from(format!("{}Errors.ts", model.module)),
            contents: ts::errors(&model, crate_name),
        },
    ];

    let cpp_files = vec![
        GeneratedFile {
            path: Utf8PathBuf::from(&ffi_header),
            contents: cpp_ffi::header(&model, ci, crate_name),
        },
        GeneratedFile {
            path: Utf8PathBuf::from(&bridge_header),
            contents: cpp_bridge::header(&model, crate_name)?,
        },
        GeneratedFile {
            path: Utf8PathBuf::from(format!("{}Bridge.cpp", model.module)),
            contents: cpp_bridge::source(&model, crate_name, &bridge_header, &ffi_header)?,
        },
    ];

    let node_files = vec![GeneratedFile {
        path: Utf8PathBuf::from(format!("{}.koffi.mjs", model.module)),
        contents: node::harness(&model, crate_name)?,
    }];

    let mut files = BTreeMap::new();
    files.insert("ts", ts_files);
    files.insert("cpp", cpp_files);
    files.insert("node", node_files);
    Ok(files)
}

/// The Nitro `HybridObject` implementations, generated against the specs
/// nitrogen produced in a previous step.
pub fn generate_hybrids(
    ci: &ComponentInterface,
    config: &NitroConfig,
    nitrogen_dir: &Utf8Path,
) -> Result<Vec<GeneratedFile>> {
    let model = Model::build(ci, config)?;
    hybrid::generate(&model, config, ci.crate_name(), nitrogen_dir)
}
