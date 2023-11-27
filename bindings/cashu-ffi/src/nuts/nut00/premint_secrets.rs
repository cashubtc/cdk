use std::ops::Deref;
use std::sync::Arc;

use cashu::nuts::nut00::wallet::PreMintSecrets as PreMintSecretsSdk;

use crate::error::Result;
use crate::{Amount, BlindedMessage, Id, Secret, SecretKey};

pub struct PreMintSecrets {
    inner: PreMintSecretsSdk,
}

impl Deref for PreMintSecrets {
    type Target = PreMintSecretsSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl PreMintSecrets {
    pub fn random(keyset_id: Arc<Id>, amount: Arc<Amount>) -> Result<Self> {
        Ok(Self {
            inner: PreMintSecretsSdk::random(
                *keyset_id.as_ref().deref(),
                *amount.as_ref().deref(),
            )?,
        })
    }

    pub fn blank(keyset_id: Arc<Id>, fee_reserve: Arc<Amount>) -> Result<Self> {
        Ok(Self {
            inner: PreMintSecretsSdk::blank(
                *keyset_id.as_ref().deref(),
                *fee_reserve.as_ref().deref(),
            )?,
        })
    }

    pub fn blinded_messages(&self) -> Vec<Arc<BlindedMessage>> {
        self.inner
            .iter()
            .map(|premint| Arc::new(premint.blinded_message.clone().into()))
            .collect()
    }

    pub fn secrets(&self) -> Vec<Arc<Secret>> {
        self.inner
            .iter()
            .map(|premint| Arc::new(premint.secret.clone().into()))
            .collect()
    }

    pub fn rs(&self) -> Vec<Arc<SecretKey>> {
        self.inner
            .iter()
            .map(|premint| Arc::new(premint.r.clone().into()))
            .collect()
    }

    pub fn amounts(&self) -> Vec<Arc<Amount>> {
        self.inner
            .iter()
            .map(|premint| Arc::new(premint.amount.into()))
            .collect()
    }
}

impl From<cashu::nuts::nut00::wallet::PreMintSecrets> for PreMintSecrets {
    fn from(inner: cashu::nuts::nut00::wallet::PreMintSecrets) -> PreMintSecrets {
        PreMintSecrets { inner }
    }
}

impl From<PreMintSecrets> for cashu::nuts::nut00::wallet::PreMintSecrets {
    fn from(blinded_messages: PreMintSecrets) -> cashu::nuts::nut00::wallet::PreMintSecrets {
        blinded_messages.inner
    }
}
