use std::ops::Deref;
use std::sync::Arc;

use cashu::nuts::nut00::BlindedMessage as BlindedMessageSdk;

use crate::nuts::nut01::public_key::PublicKey;
use crate::Amount;

pub struct BlindedMessage {
    inner: BlindedMessageSdk,
}

impl Deref for BlindedMessage {
    type Target = BlindedMessageSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl BlindedMessage {
    pub fn new(amount: Arc<Amount>, b: Arc<PublicKey>) -> Self {
        Self {
            inner: BlindedMessageSdk {
                amount: *amount.as_ref().deref(),
                b: b.as_ref().into(),
            },
        }
    }

    pub fn amount(&self) -> Arc<Amount> {
        Arc::new(self.inner.amount.into())
    }

    pub fn b(&self) -> Arc<PublicKey> {
        Arc::new(self.inner.b.clone().into())
    }
}

impl From<&BlindedMessage> for BlindedMessageSdk {
    fn from(blinded_message: &BlindedMessage) -> BlindedMessageSdk {
        blinded_message.inner.clone()
    }
}

impl From<BlindedMessageSdk> for BlindedMessage {
    fn from(inner: BlindedMessageSdk) -> BlindedMessage {
        BlindedMessage { inner }
    }
}
