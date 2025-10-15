//! Mining share wallet issue functions.
//!
//! Provides helpers to mint ecash from mining share quotes and to
//! synchronize local quote state with the mint.

use std::collections::HashMap;

use cdk_common::amount::SplitTarget;
use cdk_common::common::ProofInfo;
use cdk_common::nuts::nut12;
use cdk_common::nuts::{
    MintQuoteMiningShareResponse, MintRequest, PreMintSecrets, ProofsMethods, State,
};
use cdk_common::util::unix_time;
use cdk_common::wallet::{Transaction, TransactionDirection};
use cdk_common::{Amount, Proofs};
use tracing::instrument;

use crate::dhke::construct_proofs;
use crate::wallet::Error;
use crate::Wallet;

impl Wallet {
    /// Mint ecash for a mining share quote using stored NUT-20 credentials.
    #[instrument(skip_all)]
    pub async fn mint_mining_share(
        &self,
        quote_id: &str,
        amount: Amount,
        keyset_id: crate::nuts::Id,
        secret_key: crate::nuts::SecretKey,
    ) -> Result<Proofs, Error> {
        self.refresh_keysets().await?;

        let fee_and_amounts = self.get_keyset_fees_and_amounts_by_id(keyset_id).await?;
        let quote_record = self.localstore.get_mint_quote(quote_id).await?;
        let payment_request = quote_record.as_ref().map(|quote| quote.request.clone());

        let split_target = SplitTarget::default();
        let amount_split = amount.split_targeted(&split_target, &fee_and_amounts)?;
        let num_secrets = amount_split.len() as u32;

        tracing::debug!(
            "Incrementing keyset {} counter by {}",
            keyset_id,
            num_secrets
        );

        let new_counter = self
            .localstore
            .increment_keyset_counter(&keyset_id, num_secrets)
            .await?;
        let count = new_counter - num_secrets;

        let premint_secrets = PreMintSecrets::from_seed(
            keyset_id,
            count,
            &self.seed,
            amount,
            &split_target,
            &fee_and_amounts,
        )?;

        let mut mint_request = MintRequest {
            quote: quote_id.to_string(),
            outputs: premint_secrets.blinded_messages(),
            signature: None,
        };
        mint_request.sign(secret_key.clone())?;

        let mint_response = self.client.post_mint_mining_share(mint_request).await?;

        let keys = self.load_keyset_keys(keyset_id).await?;

        for (signature, premint) in mint_response
            .signatures
            .iter()
            .zip(&premint_secrets.secrets)
        {
            let keys = self.load_keyset_keys(signature.keyset_id).await?;
            let key = keys.amount_key(signature.amount).ok_or(Error::AmountKey)?;
            match signature.verify_dleq(key, premint.blinded_message.blinded_secret) {
                Ok(_) | Err(nut12::Error::MissingDleqProof) => (),
                Err(_) => return Err(Error::CouldNotVerifyDleq),
            }
        }

        let proofs = construct_proofs(
            mint_response.signatures,
            premint_secrets.rs(),
            premint_secrets.secrets(),
            &keys,
        )?;

        let proof_infos = proofs
            .iter()
            .map(|proof| {
                ProofInfo::new(
                    proof.clone(),
                    self.mint_url.clone(),
                    State::Unspent,
                    self.unit.clone(),
                )
            })
            .collect::<Result<Vec<ProofInfo>, _>>()?;

        self.localstore.update_proofs(proof_infos, vec![]).await?;

        self.localstore
            .add_transaction(Transaction {
                mint_url: self.mint_url.clone(),
                direction: TransactionDirection::Incoming,
                amount: proofs.total_amount()?,
                fee: Amount::ZERO,
                unit: self.unit.clone(),
                ys: proofs.ys()?,
                timestamp: unix_time(),
                memo: None,
                metadata: HashMap::new(),
                quote_id: Some(quote_id.to_string()),
                payment_request,
                payment_proof: None,
            })
            .await?;

        tracing::debug!(
            "Minted {} mining share proofs for quote {} (amount: {})",
            proofs.len(),
            quote_id,
            amount
        );

        Ok(proofs)
    }

    /// Fetch the latest state for a mining share quote and persist it locally.
    #[instrument(skip(self, quote_id))]
    pub async fn mint_quote_state_mining_share(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteMiningShareResponse<String>, Error> {
        let response = self
            .client
            .get_mint_quote_status_mining_share(quote_id)
            .await?;

        match self.localstore.get_mint_quote(quote_id).await? {
            Some(mut quote) => {
                quote.state = response.state.into();
                quote.keyset_id = Some(response.keyset_id);
                quote.amount_issued = response.amount_issued;
                quote.amount_paid = response.amount.unwrap_or(Amount::ZERO);
                self.localstore.add_mint_quote(quote).await?;
            }
            None => {
                tracing::info!("Creating local record for mining share quote {}", quote_id);

                let wallet_quote = cdk_common::wallet::MintQuote {
                    id: quote_id.to_string(),
                    mint_url: self.mint_url.clone(),
                    payment_method: cdk_common::PaymentMethod::MiningShare,
                    amount: response.amount,
                    unit: response.unit.clone().unwrap_or(self.unit.clone()),
                    request: response.request.clone(),
                    state: response.state.into(),
                    expiry: response.expiry.unwrap_or(0),
                    secret_key: None,
                    amount_issued: response.amount_issued,
                    amount_paid: response.amount.unwrap_or(Amount::ZERO),
                    keyset_id: Some(response.keyset_id),
                };

                self.localstore.add_mint_quote(wallet_quote).await?;
            }
        }

        Ok(response)
    }
}
