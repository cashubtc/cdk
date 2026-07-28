//! Parsing and resolution of `env:`/`file:` secret references.
//!
//! Secret fields in the configuration document never hold literal values.
//! They reference an environment variable (`env:VARIABLE`) or an absolute
//! file path (`file:/absolute/path`) instead. This module is the single
//! place that parses those references and reads the referenced value.

use std::fmt;
use std::path::{Path, PathBuf};

use thiserror::Error;

const ENV_SECRET_PREFIX: &str = "env:";
const FILE_SECRET_PREFIX: &str = "file:";

/// A parsed secret reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SecretRef {
    /// Read the secret from an environment variable.
    Environment(String),
    /// Read the secret from a file, exactly as stored.
    File(PathBuf),
}

/// Secret reference parsing failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum SecretRefError {
    /// The value is a literal secret instead of a reference.
    #[error("literal value; expected an `env:VARIABLE` or `file:/absolute/path` reference")]
    Literal,
    /// An `env:` reference without a variable name.
    #[error("empty environment variable name")]
    EmptyEnvironmentName,
    /// A `file:` reference without a path.
    #[error("empty file path")]
    EmptyFilePath,
    /// A `file:` reference that is not an absolute path.
    #[error("file reference {} is not an absolute path", path.display())]
    RelativeFilePath {
        /// The offending path.
        path: PathBuf,
    },
}

/// Secret resolution failures.
#[derive(Debug, Error)]
pub(crate) enum SecretResolveError {
    /// The referenced environment variable is not set.
    #[error("environment variable {name} is not set")]
    Environment {
        /// Referenced variable name.
        name: String,
    },
    /// The referenced file could not be read.
    #[error("could not read secret file {}: {source}", path.display())]
    File {
        /// Referenced file path.
        path: PathBuf,
        /// File access failure.
        #[source]
        source: std::io::Error,
    },
}

impl SecretRef {
    /// Parses a secret reference, requiring `file:` paths to be absolute.
    pub(crate) fn parse(value: &str) -> Result<Self, SecretRefError> {
        if let Some(name) = value.strip_prefix(ENV_SECRET_PREFIX) {
            if name.is_empty() {
                return Err(SecretRefError::EmptyEnvironmentName);
            }
            return Ok(Self::Environment(name.to_owned()));
        }
        if let Some(path) = value.strip_prefix(FILE_SECRET_PREFIX) {
            if path.is_empty() {
                return Err(SecretRefError::EmptyFilePath);
            }
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err(SecretRefError::RelativeFilePath { path });
            }
            return Ok(Self::File(path));
        }
        Err(SecretRefError::Literal)
    }

    /// Parses a secret reference, resolving relative `file:` paths against
    /// `base` so the result always holds an absolute path.
    pub(crate) fn parse_relative_to(value: &str, base: &Path) -> Result<Self, SecretRefError> {
        match Self::parse(value) {
            Err(SecretRefError::RelativeFilePath { path }) => Ok(Self::File(base.join(path))),
            other => other,
        }
    }

    /// Reads the referenced secret value.
    pub(crate) fn resolve(&self) -> Result<String, SecretResolveError> {
        match self {
            Self::Environment(name) => std::env::var(name)
                .map_err(|_| SecretResolveError::Environment { name: name.clone() }),
            Self::File(path) => {
                std::fs::read_to_string(path).map_err(|source| SecretResolveError::File {
                    path: path.clone(),
                    source,
                })
            }
        }
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Environment(name) => write!(f, "{ENV_SECRET_PREFIX}{name}"),
            Self::File(path) => write!(f, "{FILE_SECRET_PREFIX}{}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_environment_and_file_references() {
        assert_eq!(
            SecretRef::parse("env:CDK_MINTD_MNEMONIC").expect("environment reference"),
            SecretRef::Environment("CDK_MINTD_MNEMONIC".to_owned())
        );
        assert_eq!(
            SecretRef::parse("file:/run/secrets/mnemonic").expect("file reference"),
            SecretRef::File(PathBuf::from("/run/secrets/mnemonic"))
        );
    }

    #[test]
    fn rejects_invalid_references() {
        assert_eq!(
            SecretRef::parse("literal-secret").expect_err("literal value"),
            SecretRefError::Literal
        );
        assert_eq!(
            SecretRef::parse("env:").expect_err("empty environment name"),
            SecretRefError::EmptyEnvironmentName
        );
        assert_eq!(
            SecretRef::parse("file:").expect_err("empty file path"),
            SecretRefError::EmptyFilePath
        );
        assert_eq!(
            SecretRef::parse("file:secrets/mnemonic").expect_err("relative file path"),
            SecretRefError::RelativeFilePath {
                path: PathBuf::from("secrets/mnemonic")
            }
        );
    }

    #[test]
    fn relative_file_references_are_absolutized_against_a_base() {
        assert_eq!(
            SecretRef::parse_relative_to("file:secrets/mnemonic", Path::new("/etc/cdk-mintd"))
                .expect("relative file reference"),
            SecretRef::File(PathBuf::from("/etc/cdk-mintd/secrets/mnemonic"))
        );
        assert_eq!(
            SecretRef::parse_relative_to("file:/run/secrets/mnemonic", Path::new("/etc"))
                .expect("absolute file reference"),
            SecretRef::File(PathBuf::from("/run/secrets/mnemonic"))
        );
    }

    #[test]
    fn display_renders_the_canonical_reference() {
        assert_eq!(
            SecretRef::Environment("CDK_MINTD_MNEMONIC".to_owned()).to_string(),
            "env:CDK_MINTD_MNEMONIC"
        );
        assert_eq!(
            SecretRef::File(PathBuf::from("/run/secrets/mnemonic")).to_string(),
            "file:/run/secrets/mnemonic"
        );
    }
}
