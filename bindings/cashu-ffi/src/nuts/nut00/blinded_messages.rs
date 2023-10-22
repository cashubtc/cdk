use std::ops::Deref;
use std::sync::Arc;

use cashu::nuts::nut00::wallet::BlindedMessages as BlindedMessagesSdk;

use crate::error::Result;
use crate::{Amount, BlindedMessage, Secret, SecretKey};

pub struct BlindedMessages {
    inner: BlindedMessagesSdk,
}

impl Deref for BlindedMessages {
    type Target = BlindedMessagesSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl BlindedMessages {
    pub fn random(amount: Arc<Amount>) -> Result<Self> {
        Ok(Self {
            inner: BlindedMessagesSdk::random(*amount.as_ref().deref())?,
        })
    }

    pub fn blank(fee_reserve: Arc<Amount>) -> Result<Self> {
        Ok(Self {
            inner: BlindedMessagesSdk::blank(*fee_reserve.as_ref().deref())?,
        })
    }

    pub fn blinded_messages(&self) -> Vec<Arc<BlindedMessage>> {
        self.inner
            .blinded_messages
            .clone()
            .into_iter()
            .map(|b| Arc::new(b.into()))
            .collect()
    }

    pub fn secrets(&self) -> Vec<Arc<Secret>> {
        self.inner
            .secrets
            .clone()
            .into_iter()
            .map(|s| Arc::new(s.into()))
            .collect()
    }

    pub fn rs(&self) -> Vec<Arc<SecretKey>> {
        self.inner
            .rs
            .clone()
            .into_iter()
            .map(|s| Arc::new(s.into()))
            .collect()
    }

    pub fn amounts(&self) -> Vec<Arc<Amount>> {
        self.inner
            .amounts
            .clone()
            .into_iter()
            .map(|a| Arc::new(a.into()))
            .collect()
    }
}

impl From<cashu::nuts::nut00::wallet::BlindedMessages> for BlindedMessages {
    fn from(inner: cashu::nuts::nut00::wallet::BlindedMessages) -> BlindedMessages {
        BlindedMessages { inner }
    }
}

impl From<BlindedMessages> for cashu::nuts::nut00::wallet::BlindedMessages {
    fn from(blinded_messages: BlindedMessages) -> cashu::nuts::nut00::wallet::BlindedMessages {
        blinded_messages.inner
    }
}
