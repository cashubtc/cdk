//! Errors raised while generating bindings.

use thiserror::Error;

/// Anything that can stop generation.
#[derive(Debug, Error)]
pub enum Error {
    /// `uniffi_bindgen` failed to read metadata out of the library.
    #[error("could not read uniffi metadata from {library}: {reason}")]
    Metadata {
        /// Library the metadata was read from.
        library: String,
        /// Message from `uniffi_bindgen`.
        reason: String,
    },
    /// The library exposes no component matching the requested crate.
    #[error("library contains no uniffi component named `{crate_name}`")]
    NoSuchComponent {
        /// Crate that was requested.
        crate_name: String,
    },
    /// A UniFFI type has no Nitro equivalent.
    #[error("{context}: {type_name} cannot be represented in a Nitro binding{hint}")]
    UnsupportedType {
        /// Where the type was found.
        context: String,
        /// The offending type.
        type_name: String,
        /// Optional suggestion, already prefixed with a separator.
        hint: String,
    },
    /// A UniFFI feature has no Nitro equivalent yet.
    #[error("{context}: {feature} is not supported yet")]
    UnsupportedFeature {
        /// Where the feature was used.
        context: String,
        /// The feature name.
        feature: String,
    },
    /// A nitrogen spec header could not be understood.
    #[error("could not parse nitrogen spec {path}: {reason}")]
    SpecParse {
        /// Header that failed to parse.
        path: String,
        /// What went wrong.
        reason: String,
    },
    /// Writing an output file failed.
    #[error("could not write {path}: {reason}")]
    Write {
        /// Path being written.
        path: String,
        /// OS message.
        reason: String,
    },
    /// The `uniffi.toml` next to the crate could not be read.
    #[error("could not read config {path}: {reason}")]
    Config {
        /// Path of the config file.
        path: String,
        /// Parser message.
        reason: String,
    },
}

/// Result alias for the generator.
pub type Result<T> = std::result::Result<T, Error>;
