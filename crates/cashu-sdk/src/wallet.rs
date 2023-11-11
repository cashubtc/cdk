//! Cashu Wallet
use std::str::FromStr;

use cashu::dhke::{construct_proofs, unblind_message};
use cashu::nuts::nut00::wallet::{BlindedMessages, Token};
use cashu::nuts::nut00::{BlindedSignature, Proof, Proofs};
use cashu::nuts::nut01::Keys;
use cashu::nuts::nut03::RequestMintResponse;
use cashu::nuts::nut06::{SplitPayload, SplitRequest};
use cashu::types::{Melted, SendProofs};
use cashu::url::UncheckedUrl;
use cashu::Amount;
pub use cashu::Bolt11Invoice;
#[cfg(feature = "nut07")]
use cashu::{nuts::nut00::mint, types::ProofsStatus};
use thiserror::Error;
use tracing::warn;

use crate::client::Client;

#[derive(Debug, Error)]
pub enum Error {
    /// Insufficient Funds
    #[error("Insuddicient Funds")]
    InsufficientFunds,
    #[error("`{0}`")]
    Cashu(#[from] cashu::error::wallet::Error),
    #[error("`{0}`")]
    Client(#[from] crate::client::Error),
    /// Cashu Url Error
    #[error("`{0}`")]
    CashuUrl(#[from] cashu::url::Error),
    #[error("`{0}`")]
    Custom(String),
}

#[derive(Clone, Debug)]
pub struct Wallet<C: Client> {
    pub client: C,
    pub mint_url: UncheckedUrl,
    pub mint_keys: Keys,
    pub balance: Amount,
}

impl<C: Client> Wallet<C> {
    pub fn new(client: C, mint_url: UncheckedUrl, mint_keys: Keys) -> Self {
        Self {
            client,
            mint_url,
            mint_keys,
            balance: Amount::ZERO,
        }
    }

    // TODO: getter method for keys that if it cant get them try again

    /// Check if a proof is spent
    #[cfg(feature = "nut07")]
    pub async fn check_proofs_spent(
        &self,
        proofs: Vec<cashu::nuts::nut00::mint::Proof>,
    ) -> Result<ProofsStatus, Error> {
        use cashu::types::ProofsStatus;

        let spendable = self
            .client
            .post_check_spendable(&self.mint_url.clone().try_into()?, proofs.clone())
            .await?;

        // Separate proofs in spent and unspent based on mint response
        let (spendable, spent): (Vec<_>, Vec<_>) = proofs
            .iter()
            .zip(spendable.spendable.iter())
            .partition(|(_, &b)| b);

        Ok(ProofsStatus {
            spendable: spendable.into_iter().map(|(s, _)| s).cloned().collect(),
            spent: spent.into_iter().map(|(s, _)| s).cloned().collect(),
        })
    }

    /// Request Token Mint
    pub async fn request_mint(&self, amount: Amount) -> Result<RequestMintResponse, Error> {
        Ok(self
            .client
            .get_request_mint(&self.mint_url.clone().try_into()?, amount)
            .await?)
    }

    pub async fn mint_token(&self, amount: Amount, hash: &str) -> Result<Token, Error> {
        let proofs = self.mint(amount, hash).await?;

        let token = Token::new(self.mint_url.clone().into(), proofs, None);
        Ok(token?)
    }

    /// Mint Proofs
    pub async fn mint(&self, amount: Amount, hash: &str) -> Result<Proofs, Error> {
        let blinded_messages = BlindedMessages::random(amount)?;

        let mint_res = self
            .client
            .post_mint(
                &self.mint_url.clone().try_into()?,
                blinded_messages.clone(),
                hash,
            )
            .await?;

        let proofs = construct_proofs(
            mint_res.promises,
            blinded_messages.rs,
            blinded_messages.secrets,
            &self.mint_keys,
        )?;

        Ok(proofs)
    }

    /// Check fee
    pub async fn check_fee(&self, invoice: Bolt11Invoice) -> Result<Amount, Error> {
        Ok(self
            .client
            .post_check_fees(&self.mint_url.clone().try_into()?, invoice)
            .await?
            .fee)
    }

    /// Receive
    pub async fn receive(&self, encoded_token: &str) -> Result<Proofs, Error> {
        let token_data = Token::from_str(encoded_token)?;

        let mut proofs: Vec<Proofs> = vec![vec![]];
        for token in token_data.token {
            if token.proofs.is_empty() {
                continue;
            }

            let keys = if token.mint.to_string().eq(&self.mint_url.to_string()) {
                self.mint_keys.clone()
            } else {
                self.client.get_mint_keys(&token.mint.try_into()?).await?
            };

            // Sum amount of all proofs
            let _amount: Amount = token.proofs.iter().map(|p| p.amount).sum();

            let split_payload = self.create_split(token.proofs)?;

            let split_response = self
                .client
                .post_split(
                    &self.mint_url.clone().try_into()?,
                    split_payload.split_payload,
                )
                .await?;

            if let Some(promises) = &split_response.promises {
                // Proof to keep
                let p = construct_proofs(
                    promises.to_owned(),
                    split_payload.blinded_messages.rs,
                    split_payload.blinded_messages.secrets,
                    &keys,
                )?;
                proofs.push(p);
            } else {
                warn!("Response missing promises");
                return Err(Error::Custom("Split response missing promises".to_string()));
            }
        }
        Ok(proofs.iter().flatten().cloned().collect())
    }

    /// Create Split Payload
    fn create_split(&self, proofs: Proofs) -> Result<SplitPayload, Error> {
        let value = proofs.iter().map(|p| p.amount).sum();

        let blinded_messages = BlindedMessages::random(value)?;

        let split_payload = SplitRequest::new(proofs, blinded_messages.blinded_messages.clone());

        Ok(SplitPayload {
            blinded_messages,
            split_payload,
        })
    }

    pub fn process_split_response(
        &self,
        blinded_messages: BlindedMessages,
        promises: Vec<BlindedSignature>,
    ) -> Result<Proofs, Error> {
        let BlindedMessages {
            blinded_messages: _,
            secrets,
            rs,
            amounts: _,
        } = blinded_messages;

        let secrets: Vec<_> = secrets.iter().collect();
        let mut proofs = vec![];

        for (i, promise) in promises.iter().enumerate() {
            let a = self
                .mint_keys
                .amount_key(promise.amount)
                .unwrap()
                .to_owned();

            let blinded_c = promise.c.clone();

            let unblinded_sig = unblind_message(blinded_c, rs[i].clone().into(), a).unwrap();
            let proof = Proof {
                id: Some(promise.id),
                amount: promise.amount,
                secret: secrets[i].clone(),
                c: unblinded_sig,
            };

            proofs.push(proof);
        }

        Ok(proofs)
    }

    /// Send
    pub async fn send(&self, amount: Amount, proofs: Proofs) -> Result<SendProofs, Error> {
        let mut amount_available = Amount::ZERO;
        let mut send_proofs = SendProofs::default();

        for proof in proofs {
            let proof_value = proof.amount;
            if amount_available > amount {
                send_proofs.change_proofs.push(proof);
            } else {
                send_proofs.send_proofs.push(proof);
            }
            amount_available += proof_value;
        }

        if amount_available.lt(&amount) {
            println!("Not enough funds");
            return Err(Error::InsufficientFunds);
        }

        // If amount available is EQUAL to send amount no need to split
        if amount_available.eq(&amount) {
            return Ok(send_proofs);
        }

        let _amount_to_keep = amount_available - amount;
        let amount_to_send = amount;

        let split_payload = self.create_split(send_proofs.send_proofs)?;

        let split_response = self
            .client
            .post_split(
                &self.mint_url.clone().try_into()?,
                split_payload.split_payload,
            )
            .await?;

        // If only promises assemble proofs needed for amount
        let keep_proofs;
        let send_proofs;

        if let Some(promises) = split_response.promises {
            let proofs = construct_proofs(
                promises,
                split_payload.blinded_messages.rs,
                split_payload.blinded_messages.secrets,
                &self.mint_keys,
            )?;

            let split = amount_to_send.split();

            keep_proofs = proofs[0..split.len()].to_vec();
            send_proofs = proofs[split.len()..].to_vec();
        } else {
            return Err(Error::Custom("Invalid split response".to_string()));
        }

        // println!("Send Proofs: {:#?}", send_proofs);
        // println!("Keep Proofs: {:#?}", keep_proofs);

        Ok(SendProofs {
            change_proofs: keep_proofs,
            send_proofs,
        })
    }

    pub async fn melt(
        &self,
        invoice: Bolt11Invoice,
        proofs: Proofs,
        fee_reserve: Amount,
    ) -> Result<Melted, Error> {
        let blinded = BlindedMessages::blank(fee_reserve)?;
        let melt_response = self
            .client
            .post_melt(
                &self.mint_url.clone().try_into()?,
                proofs,
                invoice,
                Some(blinded.blinded_messages),
            )
            .await?;

        let change_proofs = match melt_response.change {
            Some(change) => Some(construct_proofs(
                change,
                blinded.rs,
                blinded.secrets,
                &self.mint_keys,
            )?),
            None => None,
        };

        let melted = Melted {
            paid: true,
            preimage: melt_response.preimage,
            change: change_proofs,
        };

        Ok(melted)
    }

    pub fn proofs_to_token(&self, proofs: Proofs, memo: Option<String>) -> Result<String, Error> {
        Ok(Token::new(self.mint_url.clone(), proofs, memo)?.convert_to_string()?)
    }
}

/*
#[cfg(test)]
mod tests {

    use std::collections::{HashMap, HashSet};

    use super::*;

    use crate::client::Client;
    use crate::mint::Mint;
    use cashu::nuts::nut04;

    #[test]
    fn test_wallet() {
        let mut mint = Mint::new(
            "supersecretsecret",
            "0/0/0/0",
            HashMap::new(),
            HashSet::new(),
            32,
        );

        let keys = mint.active_keyset_pubkeys();

        let client = Client::new("https://cashu-rs.thesimplekid.space/").unwrap();

        let wallet = Wallet::new(client, keys.keys);

        let blinded_messages = BlindedMessages::random(Amount::from_sat(64)).unwrap();

        let mint_request = nut04::MintRequest {
            outputs: blinded_messages.blinded_messages.clone(),
        };

        let res = mint.process_mint_request(mint_request).unwrap();

        let proofs = wallet
            .process_split_response(blinded_messages, res.promises)
            .unwrap();
        for proof in &proofs {
            mint.verify_proof(proof).unwrap();
        }

        let split = wallet.create_split(proofs.clone()).unwrap();

        let split_request = split.split_payload;

        let split_response = mint.process_split_request(split_request).unwrap();
        let p = split_response.promises;

        let snd_proofs = wallet
            .process_split_response(split.blinded_messages, p.unwrap())
            .unwrap();

        let mut error = false;
        for proof in &snd_proofs {
            if let Err(err) = mint.verify_proof(proof) {
                println!("{err}{:?}", serde_json::to_string(proof));
                error = true;
            }
        }

        if error {
            panic!()
        }
    }
}
*/
