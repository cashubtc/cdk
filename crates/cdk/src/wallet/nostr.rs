//! Wallet Nostr functions

use std::collections::HashSet;
use std::str::FromStr;

use nostr_sdk::nips::nip04;
use nostr_sdk::{Filter, Timestamp};
use tracing::instrument;

use super::error::Error;
use super::{util, Wallet};
use crate::amount::{Amount, SplitTarget};
use crate::nuts::{CurrencyUnit, SecretKey};

impl Wallet {
    /// Add nostr relays to client
    #[instrument(skip(self))]
    pub async fn add_nostr_relays(&self, relays: Vec<String>) -> Result<(), Error> {
        self.nostr_client.add_relays(relays).await?;
        Ok(())
    }

    /// Remove nostr relays to client
    #[instrument(skip(self))]
    pub async fn remove_nostr_relays(&self, relay: String) -> Result<(), Error> {
        self.nostr_client.remove_relay(relay).await?;
        Ok(())
    }

    /// Nostr relays
    #[instrument(skip(self))]
    pub async fn nostr_relays(&self) -> Vec<String> {
        self.nostr_client
            .relays()
            .await
            .keys()
            .map(|url| url.to_string())
            .collect()
    }

    /// Receive tokens sent to nostr pubkey via dm
    #[instrument(skip_all)]
    pub async fn nostr_receive(
        &self,
        nostr_signing_key: SecretKey,
        since: Option<u64>,
        amount_split_target: SplitTarget,
    ) -> Result<Amount, Error> {
        use nostr_sdk::{Keys, Kind};

        use crate::util::unix_time;
        use crate::Amount;

        let verifying_key = nostr_signing_key.public_key();

        let x_only_pubkey = verifying_key.x_only_public_key();

        let nostr_pubkey = nostr_sdk::PublicKey::from_hex(x_only_pubkey.to_string())?;

        let keys = Keys::from_str(&(nostr_signing_key).to_secret_hex())?;
        self.add_p2pk_signing_key(nostr_signing_key).await;

        let since = match since {
            Some(since) => Some(Timestamp::from(since)),
            None => self
                .localstore
                .get_nostr_last_checked(&verifying_key)
                .await?
                .map(|s| Timestamp::from(s as u64)),
        };

        let filter = match since {
            Some(since) => Filter::new()
                .pubkey(nostr_pubkey)
                .kind(Kind::EncryptedDirectMessage)
                .since(since),
            None => Filter::new()
                .pubkey(nostr_pubkey)
                .kind(Kind::EncryptedDirectMessage),
        };

        self.nostr_client.connect().await;

        let events = self.nostr_client.get_events_of(vec![filter], None).await?;

        let mut tokens: HashSet<String> = HashSet::new();

        for event in events {
            if event.kind() == Kind::EncryptedDirectMessage {
                if let Ok(msg) =
                    nip04::decrypt(keys.secret_key()?, event.author_ref(), event.content())
                {
                    if let Some(token) = util::token_from_text(&msg) {
                        tokens.insert(token.to_string());
                    }
                } else {
                    tracing::error!("Impossible to decrypt direct message");
                }
            }
        }

        let mut total_received = Amount::new(0, CurrencyUnit::default());
        for token in tokens.iter() {
            match self.receive(token, &amount_split_target, None).await {
                Ok(amount) => total_received.value += amount.value,
                Err(err) => {
                    tracing::error!("Could not receive token: {}", err);
                }
            }
        }

        self.localstore
            .add_nostr_last_checked(verifying_key, unix_time() as u32)
            .await?;

        Ok(total_received)
    }
}
