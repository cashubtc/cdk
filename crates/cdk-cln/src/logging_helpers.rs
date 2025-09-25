//! Logging helpers to reduce code duplication in CLN backend

use cdk_common::QuoteId;

/// Context for payment-related logging
#[derive(Debug)]
pub struct PaymentContext {
    pub quote_id: QuoteId,
    pub payment_hash: Option<String>,
    pub payment_type: PaymentType,
}

/// Payment type for context
#[derive(Debug)]
pub enum PaymentType {
    Bolt11,
    Bolt12,
}

impl PaymentContext {
    pub fn new(quote_id: &QuoteId, payment_type: PaymentType) -> Self {
        Self {
            quote_id: quote_id.clone(),
            payment_hash: None,
            payment_type,
        }
    }

    pub fn with_hash(mut self, payment_hash: &str) -> Self {
        self.payment_hash = Some(payment_hash.to_string());
        self
    }

    /// Log payment start with context
    pub fn log_start(&self, message: &str) {
        tracing::info!(
            quote_id = %self.quote_id,
            payment_type = ?self.payment_type,
            payment_hash = ?self.payment_hash,
            "{message}"
        );
    }

    /// Log payment info with context
    pub fn log_info(&self, message: &str) {
        tracing::info!(
            quote_id = %self.quote_id,
            payment_type = ?self.payment_type,
            payment_hash = ?self.payment_hash,
            "{message}"
        );
    }

    /// Log payment success with context
    pub fn log_success(&self, message: &str, total_spent: Option<u64>) {
        tracing::info!(
            quote_id = %self.quote_id,
            payment_type = ?self.payment_type,
            payment_hash = ?self.payment_hash,
            total_spent_msat = ?total_spent,
            "{message}"
        );
    }

    /// Log payment error with context
    pub fn log_error(&self, message: &str, error: &dyn std::fmt::Display) {
        tracing::error!(
            quote_id = %self.quote_id,
            payment_type = ?self.payment_type,
            payment_hash = ?self.payment_hash,
            error = %error,
            "{message}"
        );
    }

    /// Log payment debug with context
    pub fn log_debug(&self, message: &str) {
        tracing::debug!(
            quote_id = %self.quote_id,
            payment_type = ?self.payment_type,
            payment_hash = ?self.payment_hash,
            "{message}"
        );
    }
}
