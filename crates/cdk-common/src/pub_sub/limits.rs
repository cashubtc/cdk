//! Ceilings on the shared resources held by a pub/sub instance.

use std::fmt;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::Semaphore;

/// Ceilings on the shared resources one [`Pubsub`](super::Pubsub) may hold.
///
/// These are per-process rather than per-subscriber: a public mint endpoint is
/// reached by many anonymous connections, so a per-connection cap alone is
/// multiplied by the number of sockets an attacker opens.
#[derive(Debug, Clone, Copy)]
pub struct PubsubLimits {
    /// Maximum number of topic registrations held by all live subscriptions.
    pub max_topics: usize,
    /// Maximum number of backfills running concurrently.
    pub max_concurrent_backfills: usize,
    /// Maximum number of payment-backend round trips a single backfill may
    /// make. Quotes already in a final state cost nothing against it.
    pub max_quote_checks_per_backfill: usize,
    /// How long one backfill may hold its concurrency slot.
    ///
    /// The check budget bounds how many round trips a backfill makes, not how
    /// long they take, so a stalled backend would otherwise let a handful of
    /// subscriptions occupy every slot indefinitely. It is for the producer to
    /// apply, and only to work it can abandon safely: a payment transaction
    /// already begun runs to completion.
    pub backfill_timeout: Duration,
}

impl PubsubLimits {
    /// Topic budget used when none is configured.
    ///
    /// Unbounded because [`Pubsub`](super::Pubsub) also backs the wallet's local
    /// relay, where subscriptions are the wallet's own and a finite default would
    /// silently change its behaviour. A mint picks a finite budget explicitly.
    pub const DEFAULT_MAX_TOPICS: usize = usize::MAX;

    /// Concurrent backfills allowed when none is configured.
    pub const DEFAULT_MAX_CONCURRENT_BACKFILLS: usize = 32;

    /// Payment-backend checks per backfill allowed when none is configured.
    pub const DEFAULT_MAX_QUOTE_CHECKS_PER_BACKFILL: usize = 64;

    /// Time one backfill may hold its slot when none is configured.
    ///
    /// Well above what a healthy backend answers in, so it bounds a stall
    /// rather than cutting ordinary work short.
    pub const DEFAULT_BACKFILL_TIMEOUT: Duration = Duration::from_secs(30);

    /// Rejects limits the pub/sub instance could not serve.
    ///
    /// Checked when the instance is built rather than when a subscription
    /// arrives, because each of these values is applied once, to a semaphore or
    /// to a deadline: a zero budget is a permanent refusal that surfaces only as
    /// a log line, and a zero concurrency budget parks every backfill forever
    /// rather than skipping it.
    pub fn validate(&self) -> Result<(), PubsubLimitsError> {
        for (field, value) in [
            (PubsubLimitsField::MaxTopics, self.max_topics),
            (
                PubsubLimitsField::MaxConcurrentBackfills,
                self.max_concurrent_backfills,
            ),
            (
                PubsubLimitsField::MaxQuoteChecksPerBackfill,
                self.max_quote_checks_per_backfill,
            ),
        ] {
            if value == 0 {
                return Err(PubsubLimitsError::Zero { field });
            }
        }

        if self.max_concurrent_backfills > Semaphore::MAX_PERMITS {
            return Err(PubsubLimitsError::TooManyBackfills {
                maximum: Semaphore::MAX_PERMITS,
            });
        }

        if self.backfill_timeout.is_zero() {
            return Err(PubsubLimitsError::Zero {
                field: PubsubLimitsField::BackfillTimeout,
            });
        }

        Ok(())
    }
}

/// A [`PubsubLimits`] field an invalid configuration can be blamed on, so a
/// caller that exposes these limits under its own names can report the failure
/// in its own vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PubsubLimitsField {
    /// [`PubsubLimits::max_topics`].
    MaxTopics,
    /// [`PubsubLimits::max_concurrent_backfills`].
    MaxConcurrentBackfills,
    /// [`PubsubLimits::max_quote_checks_per_backfill`].
    MaxQuoteChecksPerBackfill,
    /// [`PubsubLimits::backfill_timeout`].
    BackfillTimeout,
}

impl fmt::Display for PubsubLimitsField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::MaxTopics => "max_topics",
            Self::MaxConcurrentBackfills => "max_concurrent_backfills",
            Self::MaxQuoteChecksPerBackfill => "max_quote_checks_per_backfill",
            Self::BackfillTimeout => "backfill_timeout",
        };
        f.write_str(name)
    }
}

/// Why a [`PubsubLimits`] set cannot be served.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PubsubLimitsError {
    /// A limit that has to leave at least one subscription, backfill or check
    /// possible was zero.
    #[error("{field} must be greater than zero")]
    Zero {
        /// The limit that was zero.
        field: PubsubLimitsField,
    },
    /// More concurrent backfills than the semaphore backing the budget accepts.
    #[error("max_concurrent_backfills must not exceed {maximum}")]
    TooManyBackfills {
        /// Largest budget the backfill semaphore accepts.
        maximum: usize,
    },
}

impl PubsubLimitsError {
    /// The field an operator has to change, so a caller can name it in its own
    /// configuration vocabulary without matching on every variant.
    pub const fn field(self) -> PubsubLimitsField {
        match self {
            Self::Zero { field } => field,
            Self::TooManyBackfills { .. } => PubsubLimitsField::MaxConcurrentBackfills,
        }
    }
}

impl Default for PubsubLimits {
    fn default() -> Self {
        Self {
            max_topics: Self::DEFAULT_MAX_TOPICS,
            max_concurrent_backfills: Self::DEFAULT_MAX_CONCURRENT_BACKFILLS,
            max_quote_checks_per_backfill: Self::DEFAULT_MAX_QUOTE_CHECKS_PER_BACKFILL,
            backfill_timeout: Self::DEFAULT_BACKFILL_TIMEOUT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        PubsubLimits::default()
            .validate()
            .expect("the shipped defaults must validate");
    }

    #[test]
    fn a_zero_budget_is_rejected_and_names_its_field() {
        for (limits, field) in [
            (
                PubsubLimits {
                    max_topics: 0,
                    ..PubsubLimits::default()
                },
                PubsubLimitsField::MaxTopics,
            ),
            (
                PubsubLimits {
                    max_concurrent_backfills: 0,
                    ..PubsubLimits::default()
                },
                PubsubLimitsField::MaxConcurrentBackfills,
            ),
            (
                PubsubLimits {
                    max_quote_checks_per_backfill: 0,
                    ..PubsubLimits::default()
                },
                PubsubLimitsField::MaxQuoteChecksPerBackfill,
            ),
            (
                PubsubLimits {
                    backfill_timeout: Duration::ZERO,
                    ..PubsubLimits::default()
                },
                PubsubLimitsField::BackfillTimeout,
            ),
        ] {
            let err = limits
                .validate()
                .expect_err("a zero budget leaves no work possible and must be rejected");
            assert_eq!(
                err.field(),
                field,
                "the caller must be told which field to edit: {err}"
            );
        }
    }

    #[test]
    fn a_backfill_budget_the_semaphore_cannot_hold_is_rejected() {
        let limits = PubsubLimits {
            max_concurrent_backfills: Semaphore::MAX_PERMITS + 1,
            ..PubsubLimits::default()
        };

        let err = limits
            .validate()
            .expect_err("a budget above the semaphore maximum would panic on construction");
        assert_eq!(err.field(), PubsubLimitsField::MaxConcurrentBackfills);
    }
}
