//! Cashu Wallet
use std::str::FromStr;

use bitcoin::Amount;

use crate::{
    client::Client,
    dhke::construct_proofs,
    error::Error,
    types::{
        BlindedMessages, MintKeys, Proof, ProofsStatus, RequestMintResponse, SendProofs,
        SplitPayload, SplitRequest, TokenData,
    },
};

pub struct CashuWallet {
    pub client: Client,
    pub mint_keys: MintKeys,
}

impl CashuWallet {
    pub fn new(client: Client, mint_keys: MintKeys) -> Self {
        Self { client, mint_keys }
    }

    /// Check if a proof is spent
    pub async fn check_proofs_spent(&self, proofs: Vec<Proof>) -> Result<ProofsStatus, Error> {
        let spendable = self.client.check_spendable(&proofs).await?;

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
        self.client.request_mint(amount).await
    }

    /// Mint Token
    pub async fn mint_token(
        &self,
        amount: Amount,
        payment_hash: &str,
    ) -> Result<Vec<Proof>, Error> {
        let blinded_messages = BlindedMessages::random(amount)?;

        let mint_res = self
            .client
            .mint(blinded_messages.clone(), payment_hash)
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
    pub async fn check_fee(&self, invoice: lightning_invoice::Invoice) -> Result<Amount, Error> {
        Ok(self.client.check_fees(invoice).await?.fee)
    }

    /// Receive
    pub async fn receive(&self, encoded_token: &str) -> Result<Vec<Proof>, Error> {
        let token_data = TokenData::from_str(encoded_token)?;

        let mut proofs = vec![];
        for token in token_data.token {
            if token.proofs.is_empty() {
                continue;
            }

            let keys = if token.mint.eq(&self.client.mint_url) {
                self.mint_keys.clone()
            } else {
                // FIXME:
                println!("No match");
                self.mint_keys.clone()
                // CashuMint::new(token.mint).get_keys().await.unwrap()
            };

            // Sum amount of all proofs
            let amount = token
                .proofs
                .iter()
                .fold(Amount::ZERO, |acc, p| acc + p.amount);

            let split_payload = self
                .create_split(Amount::ZERO, amount, token.proofs)
                .await?;

            let split_response = self.client.split(split_payload.split_payload).await?;

            // Proof to keep
            let keep_proofs = construct_proofs(
                split_response.fst,
                split_payload.keep_blinded_messages.rs,
                split_payload.keep_blinded_messages.secrets,
                &keys,
            )?;

            // Proofs to send
            let send_proofs = construct_proofs(
                split_response.snd,
                split_payload.send_blinded_messages.rs,
                split_payload.send_blinded_messages.secrets,
                &keys,
            )?;

            proofs.push(keep_proofs);
            proofs.push(send_proofs);
        }

        Ok(proofs.iter().flatten().cloned().collect())
    }

    /// Create Split Payload
    async fn create_split(
        &self,
        keep_amount: Amount,
        send_amount: Amount,
        proofs: Vec<Proof>,
    ) -> Result<SplitPayload, Error> {
        let keep_blinded_messages = BlindedMessages::random(keep_amount)?;
        let send_blinded_messages = BlindedMessages::random(send_amount)?;

        let outputs = {
            let mut outputs = keep_blinded_messages.blinded_messages.clone();
            outputs.extend(send_blinded_messages.blinded_messages.clone());
            outputs
        };
        let split_payload = SplitRequest {
            amount: send_amount,
            proofs,
            outputs,
        };

        Ok(SplitPayload {
            keep_blinded_messages,
            send_blinded_messages,
            split_payload,
        })
    }

    /// Send
    pub async fn send(&self, amount: Amount, proofs: Vec<Proof>) -> Result<SendProofs, Error> {
        let mut amount_available = Amount::ZERO;
        let mut send_proofs = SendProofs::default();

        for proof in proofs {
            if amount_available > amount {
                send_proofs.change_proofs.push(proof);
                break;
            } else {
                amount_available += proof.amount;
                send_proofs.send_proofs.push(proof);
            }
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

        let split_payload = self
            .create_split(amount_to_keep, amount_to_send, send_proofs.send_proofs)
            .await?;

        let split_response = self.client.split(split_payload.split_payload).await?;

        // Proof to keep
        let keep_proofs = construct_proofs(
            split_response.fst,
            split_payload.keep_blinded_messages.rs,
            split_payload.keep_blinded_messages.secrets,
            &self.mint_keys,
        )?;

        // Proofs to send
        let send_proofs = construct_proofs(
            split_response.snd,
            split_payload.send_blinded_messages.rs,
            split_payload.send_blinded_messages.secrets,
            &self.mint_keys,
        )?;

        // println!("Send Proofs: {:#?}", send_proofs);
        // println!("Keep Proofs: {:#?}", keep_proofs);

        Ok(SendProofs {
            change_proofs: keep_proofs,
            send_proofs,
        })
    }

    pub fn proofs_to_token(&self, proofs: Vec<Proof>, memo: Option<String>) -> String {
        TokenData::new(self.client.mint_url.clone(), proofs, memo).to_string()
    }
}
