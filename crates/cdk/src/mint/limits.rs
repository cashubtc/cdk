//! Resource ceilings applied by a mint.

use cdk_common::pub_sub::PubsubLimits;

/// Ceilings a mint applies to protect itself from resource exhaustion.
#[derive(Debug, Clone, Copy)]
pub struct MintLimits {
    /// Maximum number of inputs accepted in a single transaction.
    pub max_inputs: usize,
    /// Maximum number of outputs accepted in a single transaction.
    pub max_outputs: usize,
    /// Ceilings on the shared resources held by the subscription manager.
    pub pubsub: PubsubLimits,
}

impl MintLimits {
    /// Transaction input/output ceiling used when none is configured.
    pub const DEFAULT_MAX_TRANSACTION_SIZE: usize = 1000;

    /// Topic budget a mint applies when none is configured.
    ///
    /// Unlike [`PubsubLimits::DEFAULT_MAX_TOPICS`] this is finite: a mint serves
    /// anonymous subscribers, so its topic index has to be bounded.
    pub const DEFAULT_MAX_TOPICS: usize = 100_000;
}

impl Default for MintLimits {
    fn default() -> Self {
        Self {
            max_inputs: Self::DEFAULT_MAX_TRANSACTION_SIZE,
            max_outputs: Self::DEFAULT_MAX_TRANSACTION_SIZE,
            pubsub: PubsubLimits {
                max_topics: Self::DEFAULT_MAX_TOPICS,
                ..PubsubLimits::default()
            },
        }
    }
}
