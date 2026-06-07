//! Payjoin receive-session typestate wrapper.
//!
//! This tracks the pre-detection Payjoin negotiation state for an incoming
//! payment. Once a UTXO is observed, the normal `ReceiveIntent` flow takes over.

use std::marker::PhantomData;

use crate::error::Error;
use crate::storage::{BdkStorage, PayjoinReceiveSessionRecord};

/// Open Payjoin receive session marker.
#[derive(Debug, Clone)]
pub(crate) struct Open;

/// Closed Payjoin receive session marker.
#[derive(Debug, Clone)]
pub(crate) struct Closed;

/// Type-erased persisted Payjoin receive session.
#[derive(Debug, Clone)]
pub(crate) enum PayjoinReceiveSessionAny {
    /// Session can still be negotiated if Payjoin config is available.
    Open(PayjoinReceiveSession<Open>),
    /// Session is terminal and can eventually be pruned.
    Closed(PayjoinReceiveSession<Closed>),
}

/// Persisted Payjoin receive session in a particular typestate.
#[derive(Debug, Clone)]
pub(crate) struct PayjoinReceiveSession<S> {
    record: PayjoinReceiveSessionRecord,
    _state: PhantomData<S>,
}

impl<S> PayjoinReceiveSession<S> {
    /// Borrow the underlying durable record.
    pub(crate) fn record(&self) -> &PayjoinReceiveSessionRecord {
        &self.record
    }

    /// Convert into the underlying durable record.
    pub(crate) fn into_record(self) -> PayjoinReceiveSessionRecord {
        self.record
    }
}

impl PayjoinReceiveSession<Open> {
    /// Create a new open session wrapper from a freshly built record.
    pub(crate) fn new(record: PayjoinReceiveSessionRecord) -> Self {
        Self {
            record,
            _state: PhantomData,
        }
    }

    /// Whether this open session has expired.
    pub(crate) fn is_expired(&self, now: u64) -> bool {
        self.record.expires_at < now
    }

    /// Persist the open session.
    pub(crate) async fn persist(&self, storage: &BdkStorage) -> Result<(), Error> {
        storage.put_payjoin_receive_session(&self.record).await
    }

    /// Close and persist the session.
    pub(crate) async fn close(
        mut self,
        storage: &BdkStorage,
    ) -> Result<PayjoinReceiveSession<Closed>, Error> {
        self.record.closed = true;
        let closed = PayjoinReceiveSession {
            record: self.record,
            _state: PhantomData,
        };
        storage.put_payjoin_receive_session(&closed.record).await?;
        Ok(closed)
    }
}

impl PayjoinReceiveSession<Closed> {
    /// Whether this closed session has aged past the retention window.
    pub(crate) fn should_prune(&self, now: u64, retention_secs: u64) -> bool {
        self.record.expires_at.saturating_add(retention_secs) < now
    }
}

/// Reconstruct a typed Payjoin receive session from a durable record.
pub(crate) fn from_record(record: PayjoinReceiveSessionRecord) -> PayjoinReceiveSessionAny {
    if record.closed {
        PayjoinReceiveSessionAny::Closed(PayjoinReceiveSession {
            record,
            _state: PhantomData,
        })
    } else {
        PayjoinReceiveSessionAny::Open(PayjoinReceiveSession {
            record,
            _state: PhantomData,
        })
    }
}
