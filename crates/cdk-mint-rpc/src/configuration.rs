//! Transport-independent configuration management interface.

use thiserror::Error;

/// A redacted view of the configuration stored by the mint daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationSnapshot {
    /// The active configuration serialized as TOML.
    pub active_toml: String,
    /// A restart-required configuration waiting to become active.
    pub pending_toml: Option<String>,
    /// Whether the daemon must be restarted to activate pending configuration.
    pub restart_required: bool,
}

/// The result of validating or applying a complete configuration document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyConfigurationOutcome {
    /// Whether applying the configuration requires a daemon restart.
    pub restart_required: bool,
    /// Configuration field paths changed by the submitted document.
    pub changed_fields: Vec<String>,
}

/// Configuration management error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigurationError {
    /// The submitted configuration could not be parsed or validated.
    #[error("Invalid configuration: {message}")]
    Invalid {
        /// Human-readable validation failure.
        message: String,
    },
    /// The operation cannot be performed in the current configuration state.
    #[error("Configuration precondition failed: {message}")]
    FailedPrecondition {
        /// Human-readable precondition failure.
        message: String,
    },
    /// Another configuration mutation or startup activation owns the lock.
    #[error("Configuration mutation busy: {message}")]
    Busy {
        /// Human-readable contention and retry guidance.
        message: String,
    },
    /// The configuration operation failed because of an internal error.
    #[error("Configuration operation failed: {message}")]
    Internal {
        /// Human-readable internal failure.
        message: String,
    },
}

/// Held access to the database-scoped configuration mutation boundary.
///
/// The lock remains held until this guard is dropped.
pub trait ConfigurationMutationGuard: Send {}

/// Configuration manager used by management transports.
///
/// The management RPC server deliberately deals only in TOML documents and
/// transport-neutral results. Parsing, validation, persistence, redaction, and
/// restart handling remain the responsibility of the mint daemon.
#[tonic::async_trait]
pub trait ConfigurationManager: Send + Sync {
    /// Acquires the database-scoped configuration mutation lock.
    async fn acquire_configuration_mutation(
        &self,
    ) -> Result<Box<dyn ConfigurationMutationGuard>, ConfigurationError>;

    /// Returns the redacted active and pending configuration.
    async fn get_configuration(&self) -> Result<ConfigurationSnapshot, ConfigurationError>;

    /// Validates or applies a complete TOML configuration document.
    ///
    /// When `validate_only` is true, the implementation must perform no
    /// persistent mutation.
    async fn apply_configuration(
        &self,
        config_toml: String,
        validate_only: bool,
    ) -> Result<ApplyConfigurationOutcome, ConfigurationError>;

    /// Discards restart-required configuration and returns the resulting state.
    async fn discard_pending_configuration(
        &self,
    ) -> Result<ConfigurationSnapshot, ConfigurationError>;
}
