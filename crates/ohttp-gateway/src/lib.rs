pub mod cli;
pub mod gateway;
pub mod key_config;
pub mod router;

// Re-exports for easier access
pub use cli::*;
pub use gateway::*;
pub use key_config::*;
pub use router::*;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
