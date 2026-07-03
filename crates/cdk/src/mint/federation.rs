//! Federation mode scaffolding for CDK mints.
//!
//! This module is intentionally small at first. Later federation milestones will
//! attach consensus, peer networking, and threshold signing behavior to member
//! mode. Until then, the default standalone mode preserves current mint
//! behavior.

/// Defines whether a mint runs as a standalone Cashu mint or as a federation member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FederationMode {
    /// Current single-operator mint behavior.
    #[default]
    Standalone,
    /// Placeholder for consensus-backed federation member behavior.
    Member,
}

impl FederationMode {
    /// Returns true when this mode is the federation member placeholder.
    #[must_use]
    pub fn is_member(self) -> bool {
        matches!(self, Self::Member)
    }
}

#[cfg(test)]
mod tests {
    use super::FederationMode;

    #[test]
    fn federation_mode_defaults_to_standalone() {
        assert_eq!(FederationMode::default(), FederationMode::Standalone);
        assert!(!FederationMode::default().is_member());
        assert!(FederationMode::Member.is_member());
    }
}
