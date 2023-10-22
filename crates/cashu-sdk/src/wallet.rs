//! Cashu Wallet
use std::error::Error as StdError;
use std::fmt;
use std::str::FromStr;

use cashu::dhke::{construct_proofs, unblind_message};
use cashu::nuts::nut00::wallet::{BlindedMessages, Token};
use cashu::nuts::nut00::{mint, BlindedSignature, Proof, Proofs};
use cashu::nuts::nut01::Keys;
use cashu::nuts::nut03::RequestMintResponse;
use cashu::nuts::nut06::{SplitPayload, SplitRequest};
use cashu::types::{Melted, ProofsStatus, SendProofs};
use cashu::Amount;
pub use cashu::Bolt11Invoice;
use tracing::warn;

#[cfg(feature = "blocking")]
use crate::client::blocking::Client;
#[cfg(not(feature = "blocking"))]
use crate::client::Client;

#[derive(Debug)]
pub enum Error {
    /// Insufficaint Funds
    InsufficantFunds,
    Cashu(cashu::error::wallet::Error),
    Client(crate::client::Error),
    Custom(String),
}

impl StdError for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InsufficantFunds => write!(f, "Insufficant Funds"),
            Error::Cashu(err) => write!(f, "{}", err),
            Error::Client(err) => write!(f, "{}", err),
            Error::Custom(err) => write!(f, "{}", err),
        }
    }
}

impl From<cashu::error::wallet::Error> for Error {
    fn from(err: cashu::error::wallet::Error) -> Self {
        Self::Cashu(err)
    }
}

impl From<crate::client::Error> for Error {
    fn from(err: crate::client::Error) -> Error {
        Error::Client(err)
    }
}

#[derive(Clone, Debug)]
pub struct Wallet {
    pub client: Client,
    pub mint_keys: Keys,
    pub balance: Amount,
}

impl Wallet {
    pub fn new(client: Client, mint_keys: Keys) -> Self {
        Self {
            client,
            mint_keys,
            balance: Amount::ZERO,
        }
    }

    // TODO: getter method for keys that if it cant get them try again

    /// Check if a proof is spent
    #[cfg(not(feature = "blocking"))]
    pub async fn check_proofs_spent(&self, proofs: &mint::Proofs) -> Result<ProofsStatus, Error> {
        let spendable = self.client.check_spendable(proofs).await?;

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

    /// Check if a proof is spent
    #[cfg(feature = "blocking")]
    pub fn check_proofs_spent(&self, proofs: &mint::Proofs) -> Result<ProofsStatus, Error> {
        let spendable = self.client.check_spendable(proofs)?;

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
    #[cfg(not(feature = "blocking"))]
    pub async fn request_mint(&self, amount: Amount) -> Result<RequestMintResponse, Error> {
        Ok(self.client.request_mint(amount).await?)
    }

    /// Request Token Mint
    #[cfg(feature = "blocking")]
    pub fn request_mint(&self, amount: Amount) -> Result<RequestMintResponse, Error> {
        Ok(self.client.request_mint(amount)?)
    }

    /// Mint Token
    #[cfg(not(feature = "blocking"))]
    pub async fn mint_token(&self, amount: Amount, hash: &str) -> Result<Token, Error> {
        let proofs = self.mint(amount, hash).await?;

        let token = Token::new(self.client.mint_url.clone(), proofs, None);
        Ok(token?)
    }

    /// Blocking Mint Token
    #[cfg(feature = "blocking")]
    pub fn mint_token(&self, amount: Amount, hash: &str) -> Result<Token, Error> {
        let proofs = self.mint(amount, hash)?;

        let token = Token::new(self.client.client.mint_url.clone(), proofs, None);
        Ok(token?)
    }

    /// Mint Proofs
    #[cfg(not(feature = "blocking"))]
    pub async fn mint(&self, amount: Amount, hash: &str) -> Result<Proofs, Error> {
        let blinded_messages = BlindedMessages::random(amount)?;

        let mint_res = self.client.mint(blinded_messages.clone(), hash).await?;

        let proofs = construct_proofs(
            mint_res.promises,
            blinded_messages.rs,
            blinded_messages.secrets,
            &self.mint_keys,
        )?;

        Ok(proofs)
    }

    /// Blocking Mint Proofs
    #[cfg(feature = "blocking")]
    pub fn mint(&self, amount: Amount, hash: &str) -> Result<Proofs, Error> {
        let blinded_messages = BlindedMessages::random(amount)?;

        let mint_res = self.client.mint(blinded_messages.clone(), hash)?;

        let proofs = construct_proofs(
            mint_res.promises,
            blinded_messages.rs,
            blinded_messages.secrets,
            &self.mint_keys,
        )?;

        Ok(proofs)
    }

    /// Check fee
    #[cfg(not(feature = "blocking"))]
    pub async fn check_fee(&self, invoice: Bolt11Invoice) -> Result<Amount, Error> {
        Ok(self.client.check_fees(invoice).await?.fee)
    }

    /// Check fee
    #[cfg(feature = "blocking")]
    pub fn check_fee(&self, invoice: Bolt11Invoice) -> Result<Amount, Error> {
        Ok(self.client.check_fees(invoice)?.fee)
    }

    /// Receive
    #[cfg(not(feature = "blocking"))]
    pub async fn receive(&self, encoded_token: &str) -> Result<Proofs, Error> {
        let token_data = Token::from_str(encoded_token)?;

        let mut proofs: Vec<Proofs> = vec![vec![]];
        for token in token_data.token {
            if token.proofs.is_empty() {
                continue;
            }

            let keys = if token.mint.to_string().eq(&self.client.mint_url.to_string()) {
                self.mint_keys.clone()
            } else {
                Client::new(token.mint.to_string().as_str())?
                    .get_keys()
                    .await?
            };

            // Sum amount of all proofs
            let _amount: Amount = token.proofs.iter().map(|p| p.amount).sum();

            let split_payload = self.create_split(token.proofs)?;

            let split_response = self.client.split(split_payload.split_payload).await?;

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

    /// Blocking Receive
    #[cfg(feature = "blocking")]
    pub fn receive(&self, encoded_token: &str) -> Result<Proofs, Error> {
        let token_data = Token::from_str(encoded_token)?;

        let mut proofs: Vec<Proofs> = vec![vec![]];
        for token in token_data.token {
            if token.proofs.is_empty() {
                continue;
            }

            let keys = if token
                .mint
                .to_string()
                .eq(&self.client.client.mint_url.to_string())
            {
                self.mint_keys.clone()
            } else {
                Client::new(&token.mint.to_string())?.get_keys()?
            };

            // Sum amount of all proofs
            let _amount: Amount = token.proofs.iter().map(|p| p.amount).sum();

            let split_payload = self.create_split(token.proofs)?;

            let split_response = self.client.split(split_payload.split_payload)?;

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
    #[cfg(not(feature = "blocking"))]
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
            return Err(Error::InsufficantFunds);
        }

        // If amount available is EQUAL to send amount no need to split
        if amount_available.eq(&amount) {
            return Ok(send_proofs);
        }

        let _amount_to_keep = amount_available - amount;
        let amount_to_send = amount;

        let split_payload = self.create_split(send_proofs.send_proofs)?;

        let split_response = self.client.split(split_payload.split_payload).await?;

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

    /// Send
    #[cfg(feature = "blocking")]
    pub fn send(&self, amount: Amount, proofs: Proofs) -> Result<SendProofs, Error> {
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
            return Err(Error::InsufficantFunds);
        }

        // If amount available is EQUAL to send amount no need to split
        if amount_available.eq(&amount) {
            return Ok(send_proofs);
        }

        let _amount_to_keep = amount_available - amount;
        let amount_to_send = amount;

        let split_payload = self.create_split(send_proofs.send_proofs)?;

        let split_response = self.client.split(split_payload.split_payload)?;

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

    #[cfg(not(feature = "blocking"))]
    pub async fn melt(
        &self,
        invoice: Bolt11Invoice,
        proofs: Proofs,
        fee_reserve: Amount,
    ) -> Result<Melted, Error> {
        let blinded = BlindedMessages::blank(fee_reserve)?;
        let melt_response = self
            .client
            .melt(proofs, invoice, Some(blinded.blinded_messages))
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

    #[cfg(feature = "blocking")]
    pub fn melt(
        &self,
        invoice: Bolt11Invoice,
        proofs: Proofs,
        fee_reserve: Amount,
    ) -> Result<Melted, Error> {
        let blinded = BlindedMessages::blank(fee_reserve)?;
        let melt_response = self
            .client
            .melt(proofs, invoice, Some(blinded.blinded_messages))?;

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

    #[cfg(not(feature = "blocking"))]
    pub fn proofs_to_token(&self, proofs: Proofs, memo: Option<String>) -> Result<String, Error> {
        Ok(Token::new(self.client.mint_url.clone(), proofs, memo)?.convert_to_string()?)
    }

    #[cfg(feature = "blocking")]
    pub fn proofs_to_token(&self, proofs: Proofs, memo: Option<String>) -> Result<String, Error> {
        Ok(Token::new(self.client.client.mint_url.clone(), proofs, memo)?.convert_to_string()?)
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
