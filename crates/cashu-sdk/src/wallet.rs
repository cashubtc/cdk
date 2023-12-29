//! Cashu Wallet
use std::collections::HashMap;
use std::str::FromStr;

use cashu::dhke::{construct_proofs, unblind_message};
#[cfg(feature = "nut07")]
use cashu::nuts::nut00::mint;
use cashu::nuts::{
    BlindedSignature, CurrencyUnit, Keys, PreMintSecrets, PreSwap, Proof, Proofs, SwapRequest,
    Token,
};
#[cfg(feature = "nut07")]
use cashu::types::ProofsStatus;
use cashu::types::{MeltQuote, Melted, MintQuote, SendProofs};
use cashu::url::UncheckedUrl;
use cashu::Amount;
pub use cashu::Bolt11Invoice;
use thiserror::Error;
use tracing::warn;

use crate::client::Client;
use crate::utils::unix_time;

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
    #[error("Quote Expired")]
    QuoteExpired,
    #[error("Quote Unknown")]
    QuoteUnknown,
    #[error("`{0}`")]
    Custom(String),
}

#[derive(Clone, Debug)]
pub struct Wallet<C: Client> {
    pub client: C,
    pub mint_url: UncheckedUrl,
    pub mint_quotes: HashMap<String, MintQuote>,
    pub melt_quotes: HashMap<String, MeltQuote>,
    pub mint_keys: Keys,
    pub balance: Amount,
}

impl<C: Client> Wallet<C> {
    pub fn new(
        client: C,
        mint_url: UncheckedUrl,
        mint_quotes: Vec<MintQuote>,
        melt_quotes: Vec<MeltQuote>,
        mint_keys: Keys,
    ) -> Self {
        Self {
            client,
            mint_url,
            mint_keys,
            mint_quotes: mint_quotes.into_iter().map(|q| (q.id.clone(), q)).collect(),
            melt_quotes: melt_quotes.into_iter().map(|q| (q.id.clone(), q)).collect(),
            balance: Amount::ZERO,
        }
    }

    /// Check if a proof is spent
    #[cfg(feature = "nut07")]
    pub async fn check_proofs_spent(&self, proofs: Proofs) -> Result<ProofsStatus, Error> {
        let spendable = self
            .client
            .post_check_spendable(
                self.mint_url.clone().try_into()?,
                proofs
                    .clone()
                    .into_iter()
                    .map(|p| p.into())
                    .collect::<mint::Proofs>()
                    .clone(),
            )
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

    // Mint a token
    pub async fn mint_token(
        &mut self,
        amount: Amount,
        memo: Option<String>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Token, Error> {
        let quote = self
            .mint_quote(
                amount,
                unit.clone()
                    .ok_or(Error::Custom("Unit required".to_string()))?,
            )
            .await?;

        let proofs = self.mint(&quote.id).await?;

        let token = Token::new(self.mint_url.clone(), proofs, memo, unit);
        Ok(token?)
    }

    /// Mint Quote
    pub async fn mint_quote(
        &mut self,
        amount: Amount,
        unit: CurrencyUnit,
    ) -> Result<MintQuote, Error> {
        let quote_res = self
            .client
            .post_mint_quote(self.mint_url.clone().try_into()?, amount, unit.clone())
            .await?;

        let quote = MintQuote {
            id: quote_res.quote.clone(),
            amount,
            unit: unit.clone(),
            request: Bolt11Invoice::from_str(&quote_res.request).unwrap(),
            paid: quote_res.paid,
            expiry: quote_res.expiry,
        };

        self.mint_quotes
            .insert(quote_res.quote.clone(), quote.clone());

        Ok(quote)
    }

    /// Mint
    pub async fn mint(&mut self, quote_id: &str) -> Result<Proofs, Error> {
        let quote_info = self.mint_quotes.get(quote_id);

        let quote_info = if let Some(quote) = quote_info {
            if quote.expiry.le(&unix_time()) {
                return Err(Error::QuoteExpired);
            }

            quote.clone()
        } else {
            return Err(Error::QuoteUnknown);
        };

        let premint_secrets = PreMintSecrets::random((&self.mint_keys).into(), quote_info.amount)?;

        let mint_res = self
            .client
            .post_mint(
                self.mint_url.clone().try_into()?,
                quote_id,
                premint_secrets.clone(),
            )
            .await?;

        let proofs = construct_proofs(
            mint_res.signatures,
            premint_secrets.rs(),
            premint_secrets.secrets(),
            &self.mint_keys,
        )?;

        self.mint_quotes.remove(&quote_info.id);

        Ok(proofs)
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
                self.client.get_mint_keys(token.mint.try_into()?).await?
            };

            // Sum amount of all proofs
            let _amount: Amount = token.proofs.iter().map(|p| p.amount).sum();

            let pre_swap = self.create_split(None, token.proofs)?;

            let swap_response = self
                .client
                .post_split(self.mint_url.clone().try_into()?, pre_swap.split_request)
                .await?;

            // Proof to keep
            let p = construct_proofs(
                swap_response.signatures,
                pre_swap.pre_mint_secrets.rs(),
                pre_swap.pre_mint_secrets.secrets(),
                &keys,
            )?;
            proofs.push(p);
        }
        Ok(proofs.iter().flatten().cloned().collect())
    }

    /// Create Split Payload
    fn create_split(&self, amount: Option<Amount>, proofs: Proofs) -> Result<PreSwap, Error> {
        // Since split is used to get the needed combination of tokens for a specific
        // amount first blinded messages are created for the amount

        let pre_mint_secrets = if let Some(amount) = amount {
            let mut desired_messages = PreMintSecrets::random((&self.mint_keys).into(), amount)?;

            let change_amount = proofs.iter().map(|p| p.amount).sum::<Amount>() - amount;

            let change_messages = PreMintSecrets::random((&self.mint_keys).into(), change_amount)?;
            // Combine the BlindedMessages totoalling the desired amount with change
            desired_messages.combine(change_messages);
            // Sort the premint secrets to avoid finger printing
            desired_messages.sort_secrets();
            desired_messages
        } else {
            let value = proofs.iter().map(|p| p.amount).sum();

            PreMintSecrets::random((&self.mint_keys).into(), value)?
        };

        let split_request = SwapRequest::new(proofs, pre_mint_secrets.blinded_messages());

        Ok(PreSwap {
            pre_mint_secrets,
            split_request,
        })
    }

    pub fn process_split_response(
        &self,
        blinded_messages: PreMintSecrets,
        promises: Vec<BlindedSignature>,
    ) -> Result<Proofs, Error> {
        let mut proofs = vec![];

        for (promise, premint) in promises.iter().zip(blinded_messages) {
            let a = self
                .mint_keys
                .amount_key(promise.amount)
                .unwrap()
                .to_owned();

            let blinded_c = promise.c.clone();

            let unblinded_sig = unblind_message(blinded_c, premint.r.into(), a).unwrap();
            let proof = Proof {
                keyset_id: promise.keyset_id,
                amount: promise.amount,
                secret: premint.secret,
                c: unblinded_sig,
            };

            proofs.push(proof);
        }

        Ok(proofs)
    }

    /// Send
    pub async fn send(&self, amount: Amount, proofs: Proofs) -> Result<SendProofs, Error> {
        let amount_available: Amount = proofs.iter().map(|p| p.amount).sum();

        if amount_available.lt(&amount) {
            println!("Not enough funds");
            return Err(Error::InsufficientFunds);
        }

        let pre_swap = self.create_split(Some(amount), proofs)?;

        let swap_response = self
            .client
            .post_split(self.mint_url.clone().try_into()?, pre_swap.split_request)
            .await?;

        let mut keep_proofs = Proofs::new();
        let mut send_proofs = Proofs::new();

        let mut proofs = construct_proofs(
            swap_response.signatures,
            pre_swap.pre_mint_secrets.rs(),
            pre_swap.pre_mint_secrets.secrets(),
            &self.mint_keys,
        )?;

        proofs.reverse();

        for proof in proofs {
            if (proof.amount + send_proofs.iter().map(|p| p.amount).sum()).gt(&amount) {
                keep_proofs.push(proof);
            } else {
                send_proofs.push(proof);
            }
        }

        // println!("Send Proofs: {:#?}", send_proofs);
        // println!("Keep Proofs: {:#?}", keep_proofs);

        let send_amount: Amount = send_proofs.iter().map(|p| p.amount).sum();

        if send_amount.ne(&amount) {
            warn!(
                "Send amount proofs is {:?} expected {:?}",
                send_amount, amount
            );
        }

        Ok(SendProofs {
            change_proofs: keep_proofs,
            send_proofs,
        })
    }

    /// Melt Quote
    pub async fn melt_quote(
        &mut self,
        unit: CurrencyUnit,
        request: Bolt11Invoice,
    ) -> Result<MeltQuote, Error> {
        let quote_res = self
            .client
            .post_melt_quote(
                self.mint_url.clone().try_into()?,
                unit.clone(),
                request.clone(),
            )
            .await?;

        let quote = MeltQuote {
            id: quote_res.quote,
            amount: quote_res.amount.into(),
            request,
            unit,
            fee_reserve: quote_res.fee_reserve.into(),
            paid: quote_res.paid,
            expiry: quote_res.expiry,
        };

        self.melt_quotes.insert(quote.id.clone(), quote.clone());

        Ok(quote)
    }

    /// Melt
    pub async fn melt(&self, quote_id: &str, proofs: Proofs) -> Result<Melted, Error> {
        let quote_info = self.melt_quotes.get(quote_id);

        let quote_info = if let Some(quote) = quote_info {
            if quote.expiry.le(&unix_time()) {
                return Err(Error::QuoteExpired);
            }

            quote.clone()
        } else {
            return Err(Error::QuoteUnknown);
        };

        let blinded = PreMintSecrets::blank((&self.mint_keys).into(), quote_info.fee_reserve)?;

        let melt_response = self
            .client
            .post_melt(
                self.mint_url.clone().try_into()?,
                quote_id.to_string(),
                proofs,
                Some(blinded.blinded_messages()),
            )
            .await?;

        let change_proofs = match melt_response.change {
            Some(change) => Some(construct_proofs(
                change,
                blinded.rs(),
                blinded.secrets(),
                &self.mint_keys,
            )?),
            None => None,
        };

        let melted = Melted {
            paid: true,
            preimage: melt_response.payment_preimage,
            change: change_proofs,
        };

        Ok(melted)
    }

    pub fn proofs_to_token(
        &self,
        proofs: Proofs,
        memo: Option<String>,
        unit: Option<CurrencyUnit>,
    ) -> Result<String, Error> {
        Ok(Token::new(self.mint_url.clone(), proofs, memo, unit)?.to_string())
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
