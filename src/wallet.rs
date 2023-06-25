//! Cashu Wallet
use std::str::FromStr;

use crate::dhke::unblind_message;
use crate::nuts::nut00::{mint, BlindedMessages, BlindedSignature, Proof, Proofs, Token};
use crate::nuts::nut01::Keys;
use crate::nuts::nut03::RequestMintResponse;
use crate::nuts::nut06::{SplitPayload, SplitRequest};
use crate::types::{Melted, ProofsStatus, SendProofs};
pub use crate::Invoice;
use crate::{client::Client, dhke::construct_proofs, error::Error};

use crate::amount::Amount;

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

    /// Request Token Mint
    pub async fn request_mint(&self, amount: Amount) -> Result<RequestMintResponse, Error> {
        Ok(self.client.request_mint(amount).await?)
    }

    /// Mint Token
    pub async fn mint_token(&self, amount: Amount, hash: &str) -> Result<Token, Error> {
        let proofs = self.mint(amount, hash).await?;

        let token = Token::new(self.client.mint_url.clone(), proofs, None);
        Ok(token)
    }

    /// Mint Proofs
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

    /// Check fee
    pub async fn check_fee(&self, invoice: Invoice) -> Result<Amount, Error> {
        Ok(self.client.check_fees(invoice).await?.fee)
    }

    /// Receive
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
                Client::new(token.mint.as_str())?.get_keys().await?
            };

            // Sum amount of all proofs
            let amount = token
                .proofs
                .iter()
                .fold(Amount::ZERO, |acc, p| acc + p.amount);

            let split_payload = self.create_split(Amount::ZERO, amount, token.proofs)?;

            let split_response = self.client.split(split_payload.split_payload).await?;

            if let Some(promises) = &split_response.promises {
                // Proof to keep
                let p = construct_proofs(
                    split_response.promises.unwrap(),
                    split_payload.keep_blinded_messages.rs,
                    split_payload.keep_blinded_messages.secrets,
                    &keys,
                )?;
                proofs.push(p);
            } else {
                // Proof to keep
                let keep_proofs = construct_proofs(
                    split_response.fst.unwrap(),
                    split_payload.keep_blinded_messages.rs,
                    split_payload.keep_blinded_messages.secrets,
                    &keys,
                )?;

                // Proofs to send
                let send_proofs = construct_proofs(
                    split_response.snd.unwrap(),
                    split_payload.send_blinded_messages.rs,
                    split_payload.send_blinded_messages.secrets,
                    &keys,
                )?;

                proofs.push(send_proofs);
                proofs.push(keep_proofs);
            }
        }
        Ok(proofs.iter().flatten().cloned().collect())
    }

    /// Create Split Payload
    fn create_split(
        &self,
        keep_amount: Amount,
        send_amount: Amount,
        proofs: Proofs,
    ) -> Result<SplitPayload, Error> {
        let keep_blinded_messages = BlindedMessages::random(keep_amount)?;
        let send_blinded_messages = BlindedMessages::random(send_amount)?;

        let outputs = {
            let mut outputs = keep_blinded_messages.blinded_messages.clone();
            outputs.extend(send_blinded_messages.blinded_messages.clone());
            outputs
        };

        let split_payload = SplitRequest {
            amount: Some(send_amount),
            proofs,
            outputs,
        };

        Ok(SplitPayload {
            keep_blinded_messages,
            send_blinded_messages,
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
                id: Some(promise.id.clone()),
                amount: promise.amount,
                secret: secrets[i].clone(),
                c: unblinded_sig,
                script: None,
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
            return Err(Error::InsufficantFunds);
        }

        // If amount available is EQUAL to send amount no need to split
        if amount_available.eq(&amount) {
            return Ok(send_proofs);
        }

        let amount_to_keep = amount_available - amount;
        let amount_to_send = amount;

        // TODO: Will need to change https://github.com/cashubtc/cashu/pull/263/files
        let split_payload =
            self.create_split(amount_to_keep, amount_to_send, send_proofs.send_proofs)?;

        let split_response = self.client.split(split_payload.split_payload).await?;

        // If only prmises assemble proofs needed for amount

        let keep_proofs;
        let send_proofs;

        if let Some(promises) = split_response.promises {
            let proofs = construct_proofs(
                promises,
                split_payload.keep_blinded_messages.rs,
                split_payload.keep_blinded_messages.secrets,
                &self.mint_keys,
            )?;

            let split = amount_to_send.split();

            keep_proofs = proofs[0..split.len()].to_vec();
            send_proofs = proofs[split.len()..].to_vec();
        } else if let (Some(fst), Some(snd)) = (split_response.fst, split_response.snd) {
            // Proof to keep
            keep_proofs = construct_proofs(
                fst,
                split_payload.keep_blinded_messages.rs,
                split_payload.keep_blinded_messages.secrets,
                &self.mint_keys,
            )?;

            // Proofs to send
            send_proofs = construct_proofs(
                snd,
                split_payload.send_blinded_messages.rs,
                split_payload.send_blinded_messages.secrets,
                &self.mint_keys,
            )?;
        } else {
            return Err(Error::CustomError("Invalid split response".to_string()));
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
        invoice: Invoice,
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

    pub fn proofs_to_token(&self, proofs: Proofs, memo: Option<String>) -> Result<String, Error> {
        Token::new(self.client.mint_url.clone(), proofs, memo).convert_to_string()
    }
}

#[cfg(test)]
mod tests {

    use std::collections::{HashMap, HashSet};

    use super::*;

    use crate::client::Client;
    use crate::mint::Mint;
    use crate::nuts::nut04;

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

        let split = wallet
            .create_split(Amount::from_sat(24), Amount::from_sat(40), proofs.clone())
            .unwrap();

        let split_request = split.split_payload;

        let split_response = mint.process_split_request(split_request).unwrap();
        let p = split_response.snd;

        let snd_proofs = wallet
            .process_split_response(split.send_blinded_messages, p.unwrap())
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
