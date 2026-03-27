//! WalletTrait implementation for the FFI Wallet.
//!
//! Implements `cdk_common::wallet::Wallet` for the FFI `Wallet` type,
//! converting between FFI types and CDK types at the boundary.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
use cdk_common::bitcoin;
use cdk_common::wallet::Wallet as WalletTraitDef;

use crate::error::FfiError;
use crate::types::*;
use crate::wallet::Wallet;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl WalletTraitDef for Wallet {
    type Error = FfiError;
    type Amount = Amount;
    type MintUrl = MintUrl;
    type CurrencyUnit = CurrencyUnit;
    type MintInfo = MintInfo;
    type KeySetInfo = KeySetInfo;
    type MintQuote = MintQuote;
    type MeltQuote = MeltQuote;
    type PaymentMethod = PaymentMethod;
    type MeltOptions = MeltOptions;
    type OperationId = String;
    type PreparedSend<'a> = Arc<PreparedSend>;
    type PreparedMelt<'a> = PreparedMelt;
    type Subscription = Arc<crate::types::ActiveSubscription>;
    type SubscribeParams = crate::types::SubscribeParams;

    fn mint_url(&self) -> Self::MintUrl {
        self.inner().mint_url.clone().into()
    }

    fn unit(&self) -> Self::CurrencyUnit {
        self.inner().unit.clone().into()
    }

    async fn total_balance(&self) -> Result<Self::Amount, Self::Error> {
        let balance = WalletTraitDef::total_balance(self.inner().as_ref()).await?;
        Ok(balance.into())
    }

    async fn total_pending_balance(&self) -> Result<Self::Amount, Self::Error> {
        let balance = WalletTraitDef::total_pending_balance(self.inner().as_ref()).await?;
        Ok(balance.into())
    }

    async fn total_reserved_balance(&self) -> Result<Self::Amount, Self::Error> {
        let balance = WalletTraitDef::total_reserved_balance(self.inner().as_ref()).await?;
        Ok(balance.into())
    }

    async fn fetch_mint_info(&self) -> Result<Option<Self::MintInfo>, Self::Error> {
        let info = WalletTraitDef::fetch_mint_info(self.inner().as_ref()).await?;
        Ok(info.map(Into::into))
    }

    async fn load_mint_info(&self) -> Result<Self::MintInfo, Self::Error> {
        let info = WalletTraitDef::load_mint_info(self.inner().as_ref()).await?;
        Ok(info.into())
    }

    async fn refresh_keysets(&self) -> Result<Vec<Self::KeySetInfo>, Self::Error> {
        let keysets = WalletTraitDef::refresh_keysets(self.inner().as_ref()).await?;
        Ok(keysets.into_iter().map(Into::into).collect())
    }

    async fn get_active_keyset(&self) -> Result<Self::KeySetInfo, Self::Error> {
        let keyset = WalletTraitDef::get_active_keyset(self.inner().as_ref()).await?;
        Ok(keyset.into())
    }

    async fn mint_quote(
        &self,
        method: Self::PaymentMethod,
        amount: Option<Self::Amount>,
        description: Option<String>,
        extra: Option<String>,
    ) -> Result<Self::MintQuote, Self::Error> {
        let quote = WalletTraitDef::mint_quote(
            self.inner().as_ref(),
            method.into(),
            amount.map(Into::into),
            description,
            extra,
        )
        .await?;
        Ok(quote.into())
    }

    async fn melt_quote(
        &self,
        method: Self::PaymentMethod,
        request: String,
        options: Option<Self::MeltOptions>,
        extra: Option<String>,
    ) -> Result<Self::MeltQuote, Self::Error> {
        let quote = WalletTraitDef::melt_quote(
            self.inner().as_ref(),
            method.into(),
            request,
            options.map(Into::into),
            extra,
        )
        .await?;
        Ok(quote.into())
    }

    async fn list_transactions(
        &self,
        direction: Option<cdk_common::wallet::TransactionDirection>,
    ) -> Result<Vec<cdk_common::wallet::Transaction>, Self::Error> {
        let txs = WalletTraitDef::list_transactions(self.inner().as_ref(), direction).await?;
        Ok(txs)
    }

    async fn get_transaction(
        &self,
        id: cdk_common::wallet::TransactionId,
    ) -> Result<Option<cdk_common::wallet::Transaction>, Self::Error> {
        let tx = WalletTraitDef::get_transaction(self.inner().as_ref(), id).await?;
        Ok(tx)
    }

    async fn get_proofs_for_transaction(
        &self,
        id: cdk_common::wallet::TransactionId,
    ) -> Result<cdk_common::Proofs, Self::Error> {
        let proofs = WalletTraitDef::get_proofs_for_transaction(self.inner().as_ref(), id).await?;
        Ok(proofs)
    }

    async fn revert_transaction(
        &self,
        id: cdk_common::wallet::TransactionId,
    ) -> Result<(), Self::Error> {
        WalletTraitDef::revert_transaction(self.inner().as_ref(), id).await?;
        Ok(())
    }

    async fn check_all_pending_proofs(&self) -> Result<Self::Amount, Self::Error> {
        let amount = WalletTraitDef::check_all_pending_proofs(self.inner().as_ref()).await?;
        Ok(amount.into())
    }

    async fn check_proofs_spent(
        &self,
        proofs: cdk_common::Proofs,
    ) -> Result<Vec<cdk_common::nuts::nut07::ProofState>, Self::Error> {
        let states = WalletTraitDef::check_proofs_spent(self.inner().as_ref(), proofs).await?;
        Ok(states)
    }

    async fn get_keyset_fees_by_id(
        &self,
        keyset_id: cdk_common::nuts::Id,
    ) -> Result<u64, Self::Error> {
        let fee = WalletTraitDef::get_keyset_fees_by_id(self.inner().as_ref(), keyset_id).await?;
        Ok(fee)
    }

    async fn calculate_fee(
        &self,
        proof_count: u64,
        keyset_id: cdk_common::nuts::Id,
    ) -> Result<Self::Amount, Self::Error> {
        let fee =
            WalletTraitDef::calculate_fee(self.inner().as_ref(), proof_count, keyset_id).await?;
        Ok(fee.into())
    }

    async fn receive(
        &self,
        encoded_token: &str,
        options: cdk_common::wallet::ReceiveOptions,
    ) -> Result<Self::Amount, Self::Error> {
        let amount = WalletTraitDef::receive(self.inner().as_ref(), encoded_token, options).await?;
        Ok(amount.into())
    }

    async fn receive_proofs(
        &self,
        proofs: cdk_common::Proofs,
        options: cdk_common::wallet::ReceiveOptions,
        memo: Option<String>,
        token: Option<String>,
    ) -> Result<Self::Amount, Self::Error> {
        let amount =
            WalletTraitDef::receive_proofs(self.inner().as_ref(), proofs, options, memo, token)
                .await?;
        Ok(amount.into())
    }

    async fn prepare_send(
        &self,
        amount: Self::Amount,
        options: cdk_common::wallet::SendOptions,
    ) -> Result<Arc<PreparedSend>, Self::Error> {
        let prepared = self.inner().prepare_send(amount.into(), options).await?;
        Ok(Arc::new(PreparedSend::new(self.inner().clone(), &prepared)))
    }

    async fn get_pending_sends(&self) -> Result<Vec<String>, Self::Error> {
        let ids = WalletTraitDef::get_pending_sends(self.inner().as_ref()).await?;
        Ok(ids.into_iter().map(|id| id.to_string()).collect())
    }

    async fn revoke_send(&self, operation_id: String) -> Result<Self::Amount, Self::Error> {
        let uuid = uuid::Uuid::parse_str(&operation_id)
            .map_err(|e| FfiError::internal(format!("Invalid operation ID: {}", e)))?;
        let amount = WalletTraitDef::revoke_send(self.inner().as_ref(), uuid).await?;
        Ok(amount.into())
    }

    async fn check_send_status(&self, operation_id: String) -> Result<bool, Self::Error> {
        let uuid = uuid::Uuid::parse_str(&operation_id)
            .map_err(|e| FfiError::internal(format!("Invalid operation ID: {}", e)))?;
        let claimed = WalletTraitDef::check_send_status(self.inner().as_ref(), uuid).await?;
        Ok(claimed)
    }

    async fn mint(
        &self,
        quote_id: &str,
        split_target: cdk_common::amount::SplitTarget,
        spending_conditions: Option<cdk_common::nuts::SpendingConditions>,
    ) -> Result<cdk_common::Proofs, Self::Error> {
        let proofs = WalletTraitDef::mint(
            self.inner().as_ref(),
            quote_id,
            split_target,
            spending_conditions,
        )
        .await?;
        Ok(proofs)
    }

    async fn check_mint_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<Self::MintQuote, Self::Error> {
        let quote =
            WalletTraitDef::check_mint_quote_status(self.inner().as_ref(), quote_id).await?;
        Ok(quote.into())
    }

    async fn fetch_mint_quote(
        &self,
        quote_id: &str,
        payment_method: Option<Self::PaymentMethod>,
    ) -> Result<Self::MintQuote, Self::Error> {
        let method = payment_method.map(Into::into);
        let quote =
            WalletTraitDef::fetch_mint_quote(self.inner().as_ref(), quote_id, method).await?;
        Ok(quote.into())
    }

    async fn prepare_melt(
        &self,
        quote_id: &str,
        metadata: HashMap<String, String>,
    ) -> Result<PreparedMelt, Self::Error> {
        let prepared = self.inner().prepare_melt(quote_id, metadata).await?;
        Ok(PreparedMelt::new(self.inner().clone(), &prepared))
    }

    async fn prepare_melt_proofs(
        &self,
        quote_id: &str,
        proofs: cdk_common::Proofs,
        metadata: HashMap<String, String>,
    ) -> Result<PreparedMelt, Self::Error> {
        let prepared = self
            .inner()
            .prepare_melt_proofs(quote_id, proofs, metadata)
            .await?;
        Ok(PreparedMelt::new(self.inner().clone(), &prepared))
    }

    async fn swap(
        &self,
        amount: Option<Self::Amount>,
        split_target: cdk_common::amount::SplitTarget,
        input_proofs: cdk_common::Proofs,
        spending_conditions: Option<cdk_common::nuts::SpendingConditions>,
        include_fees: bool,
        use_p2bk: bool,
    ) -> Result<Option<cdk_common::Proofs>, Self::Error> {
        let result = WalletTraitDef::swap(
            self.inner().as_ref(),
            amount.map(Into::into),
            split_target,
            input_proofs,
            spending_conditions,
            include_fees,
            use_p2bk,
        )
        .await?;
        Ok(result)
    }

    async fn set_cat(&self, cat: String) -> Result<(), Self::Error> {
        WalletTraitDef::set_cat(self.inner().as_ref(), cat).await?;
        Ok(())
    }

    async fn set_refresh_token(&self, refresh_token: String) -> Result<(), Self::Error> {
        WalletTraitDef::set_refresh_token(self.inner().as_ref(), refresh_token).await?;
        Ok(())
    }

    async fn refresh_access_token(&self) -> Result<(), Self::Error> {
        WalletTraitDef::refresh_access_token(self.inner().as_ref()).await?;
        Ok(())
    }

    async fn mint_blind_auth(
        &self,
        amount: Self::Amount,
    ) -> Result<cdk_common::Proofs, Self::Error> {
        let proofs = WalletTraitDef::mint_blind_auth(self.inner().as_ref(), amount.into()).await?;
        Ok(proofs)
    }

    async fn get_unspent_auth_proofs(
        &self,
    ) -> Result<Vec<cdk_common::nuts::AuthProof>, Self::Error> {
        let proofs = WalletTraitDef::get_unspent_auth_proofs(self.inner().as_ref()).await?;
        Ok(proofs)
    }

    async fn restore(&self) -> Result<cdk_common::wallet::Restored, Self::Error> {
        let restored = WalletTraitDef::restore(self.inner().as_ref()).await?;
        Ok(restored)
    }

    async fn verify_token_dleq(&self, token_str: &str) -> Result<(), Self::Error> {
        WalletTraitDef::verify_token_dleq(self.inner().as_ref(), token_str).await?;
        Ok(())
    }

    async fn pay_request(
        &self,
        request: cdk_common::nuts::nut18::PaymentRequest,
        custom_amount: Option<Self::Amount>,
    ) -> Result<(), Self::Error> {
        WalletTraitDef::pay_request(
            self.inner().as_ref(),
            request,
            custom_amount.map(Into::into),
        )
        .await?;
        Ok(())
    }

    async fn subscribe_mint_quote_state(
        &self,
        quote_ids: Vec<String>,
        method: Self::PaymentMethod,
    ) -> Result<Arc<crate::types::ActiveSubscription>, Self::Error> {
        let cdk_sub = WalletTraitDef::subscribe_mint_quote_state(
            self.inner().as_ref(),
            quote_ids,
            method.into(),
        )
        .await?;
        let sub_id = uuid::Uuid::new_v4().to_string();
        Ok(Arc::new(crate::types::ActiveSubscription::new(
            cdk_sub, sub_id,
        )))
    }

    fn set_metadata_cache_ttl(&self, ttl_secs: Option<u64>) {
        let ttl = ttl_secs.map(std::time::Duration::from_secs);
        self.inner().set_metadata_cache_ttl(ttl);
    }

    async fn subscribe(
        &self,
        params: crate::types::SubscribeParams,
    ) -> Result<Arc<crate::types::ActiveSubscription>, Self::Error> {
        let cdk_params: cdk_common::subscription::WalletParams = params.into();
        let sub_id = cdk_params.id.to_string();
        let active_sub = WalletTraitDef::subscribe(self.inner().as_ref(), cdk_params).await?;
        Ok(Arc::new(crate::types::ActiveSubscription::new(
            active_sub, sub_id,
        )))
    }

    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn melt_bip353_quote(
        &self,
        bip353_address: &str,
        amount_msat: Self::Amount,
        network: bitcoin::Network,
    ) -> Result<Self::MeltQuote, Self::Error> {
        let cdk_amount: cdk_common::Amount = amount_msat.into();
        let quote = self
            .inner()
            .melt_bip353_quote(bip353_address, cdk_amount, network)
            .await?;
        Ok(quote.into())
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn melt_lightning_address_quote(
        &self,
        lightning_address: &str,
        amount_msat: Self::Amount,
    ) -> Result<Self::MeltQuote, Self::Error> {
        let cdk_amount: cdk_common::Amount = amount_msat.into();
        let quote = self
            .inner()
            .melt_lightning_address_quote(lightning_address, cdk_amount)
            .await?;
        Ok(quote.into())
    }

    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn melt_human_readable_quote(
        &self,
        address: &str,
        amount_msat: Self::Amount,
        network: bitcoin::Network,
    ) -> Result<Self::MeltQuote, Self::Error> {
        let cdk_amount: cdk_common::Amount = amount_msat.into();
        let quote = self
            .inner()
            .melt_human_readable_quote(address, cdk_amount, network)
            .await?;
        Ok(quote.into())
    }

    async fn get_proofs_by_states(
        &self,
        states: Vec<cdk_common::nuts::State>,
    ) -> Result<cdk_common::Proofs, Self::Error> {
        let proofs = WalletTraitDef::get_proofs_by_states(self.inner().as_ref(), states).await?;
        Ok(proofs)
    }

    /// generates and stores public key in database
    async fn generate_public_key(&self) -> Result<cdk::nuts::PublicKey, Self::Error> {
        let quote = WalletTraitDef::generate_public_key(self.inner().as_ref()).await?;
        Ok(quote)
    }

    /// gets public key by it's hex value
    async fn get_public_key(
        &self,
        pubkey: &cdk::nuts::PublicKey,
    ) -> Result<Option<cdk_common::wallet::P2PKSigningKey>, Self::Error> {
        let pubkey = WalletTraitDef::get_public_key(self.inner().as_ref(), pubkey).await?;
        Ok(pubkey)
    }

    /// gets list of stored public keys in database
    async fn get_public_keys(
        &self,
    ) -> Result<Vec<cdk_common::wallet::P2PKSigningKey>, Self::Error> {
        let pubkeys = WalletTraitDef::get_public_keys(self.inner().as_ref()).await?;
        Ok(pubkeys)
    }

    /// Gets the latest generated P2PK signing key (most recently created)
    async fn get_latest_public_key(
        &self,
    ) -> Result<Option<cdk_common::wallet::P2PKSigningKey>, Self::Error> {
        let pubkey = WalletTraitDef::get_latest_public_key(self.inner().as_ref()).await?;
        Ok(pubkey)
    }

    /// try to get secret key from p2pk signing key in localstore
    async fn get_signing_key(
        &self,
        pubkey: &cdk::nuts::PublicKey,
    ) -> Result<Option<cdk::nuts::SecretKey>, Self::Error> {
        let signing_key = WalletTraitDef::get_signing_key(self.inner().as_ref(), pubkey).await?;
        Ok(signing_key)
    }
}
