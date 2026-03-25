//! WalletTrait implementation for the CDK Wallet.

use std::collections::HashMap;
use std::str::FromStr;

use async_trait::async_trait;
use cdk_common::amount::SplitTarget;
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::nut07::ProofState;
use cdk_common::nuts::nut18::PaymentRequest;
use cdk_common::nuts::{
    AuthProof, CurrencyUnit, Id, KeySetInfo, MeltOptions, MintInfo, PaymentMethod, Proofs,
    SpendingConditions,
};
use cdk_common::subscription::WalletParams;
use cdk_common::wallet::{
    MeltQuote, MintQuote, ReceiveOptions, Restored, SendOptions, Transaction, TransactionDirection,
    TransactionId, Wallet as WalletTrait,
};
use cdk_common::Amount;
use tracing::instrument;
use uuid::Uuid;

use crate::wallet::subscription::ActiveSubscription;
use crate::Error;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl WalletTrait for super::Wallet {
    type Error = Error;
    type Amount = Amount;
    type MintUrl = MintUrl;
    type CurrencyUnit = CurrencyUnit;
    type MintInfo = MintInfo;
    type KeySetInfo = KeySetInfo;
    type MintQuote = MintQuote;
    type MeltQuote = MeltQuote;
    type PaymentMethod = PaymentMethod;
    type MeltOptions = MeltOptions;
    type OperationId = Uuid;
    type PreparedSend<'a> = super::send::PreparedSend<'a>;
    type PreparedMelt<'a> = super::melt::PreparedMelt<'a>;
    type Subscription = ActiveSubscription;
    type SubscribeParams = WalletParams;

    fn mint_url(&self) -> MintUrl {
        self.mint_url.clone()
    }

    fn unit(&self) -> CurrencyUnit {
        self.unit.clone()
    }

    #[instrument(skip(self))]
    async fn total_balance(&self) -> Result<Amount, Self::Error> {
        self.total_balance().await
    }

    #[instrument(skip(self))]
    async fn total_pending_balance(&self) -> Result<Amount, Self::Error> {
        self.total_pending_balance().await
    }

    #[instrument(skip(self))]
    async fn total_reserved_balance(&self) -> Result<Amount, Self::Error> {
        self.total_reserved_balance().await
    }

    #[instrument(skip(self))]
    async fn fetch_mint_info(&self) -> Result<Option<MintInfo>, Self::Error> {
        self.fetch_mint_info().await
    }

    #[instrument(skip(self))]
    async fn load_mint_info(&self) -> Result<MintInfo, Self::Error> {
        self.load_mint_info().await
    }

    #[instrument(skip(self))]
    async fn refresh_keysets(&self) -> Result<Vec<KeySetInfo>, Self::Error> {
        self.refresh_keysets().await
    }

    #[instrument(skip(self))]
    async fn get_active_keyset(&self) -> Result<KeySetInfo, Self::Error> {
        self.get_active_keyset().await
    }

    #[instrument(skip(self, method))]
    async fn mint_quote(
        &self,
        method: PaymentMethod,
        amount: Option<Amount>,
        description: Option<String>,
        extra: Option<String>,
    ) -> Result<MintQuote, Self::Error> {
        self.mint_quote(method, amount, description, extra).await
    }

    #[instrument(skip(self, request, options, extra))]
    async fn melt_quote(
        &self,
        method: PaymentMethod,
        request: String,
        options: Option<MeltOptions>,
        extra: Option<String>,
    ) -> Result<MeltQuote, Self::Error> {
        self.melt_quote(method, request, options, extra).await
    }

    #[instrument(skip(self))]
    async fn list_transactions(
        &self,
        direction: Option<TransactionDirection>,
    ) -> Result<Vec<Transaction>, Self::Error> {
        self.list_transactions(direction).await
    }

    #[instrument(skip(self))]
    async fn get_transaction(&self, id: TransactionId) -> Result<Option<Transaction>, Self::Error> {
        self.get_transaction(id).await
    }

    #[instrument(skip(self))]
    async fn get_proofs_for_transaction(&self, id: TransactionId) -> Result<Proofs, Self::Error> {
        self.get_proofs_for_transaction(id).await
    }

    #[instrument(skip(self))]
    async fn revert_transaction(&self, id: TransactionId) -> Result<(), Self::Error> {
        self.revert_transaction(id).await
    }

    #[instrument(skip(self))]
    async fn check_all_pending_proofs(&self) -> Result<Amount, Self::Error> {
        self.check_all_pending_proofs().await
    }

    #[instrument(skip(self, proofs))]
    async fn check_proofs_spent(&self, proofs: Proofs) -> Result<Vec<ProofState>, Self::Error> {
        self.check_proofs_spent(proofs).await
    }

    #[instrument(skip(self))]
    async fn get_keyset_fees_by_id(&self, keyset_id: Id) -> Result<u64, Self::Error> {
        self.get_keyset_fees_by_id(keyset_id).await
    }

    #[instrument(skip(self))]
    async fn calculate_fee(&self, proof_count: u64, keyset_id: Id) -> Result<Amount, Self::Error> {
        self.calculate_fee(proof_count, keyset_id).await
    }

    #[instrument(skip(self, encoded_token, options))]
    async fn receive(
        &self,
        encoded_token: &str,
        options: ReceiveOptions,
    ) -> Result<Amount, Self::Error> {
        self.receive(encoded_token, options).await
    }

    #[instrument(skip(self, proofs, options))]
    async fn receive_proofs(
        &self,
        proofs: Proofs,
        options: ReceiveOptions,
        memo: Option<String>,
        token: Option<String>,
    ) -> Result<Amount, Self::Error> {
        self.receive_proofs(proofs, options, memo, token).await
    }

    #[instrument(skip(self, options))]
    async fn prepare_send(
        &self,
        amount: Amount,
        options: SendOptions,
    ) -> Result<super::send::PreparedSend<'_>, Self::Error> {
        self.prepare_send(amount, options).await
    }

    #[instrument(skip(self))]
    async fn get_pending_sends(&self) -> Result<Vec<Uuid>, Self::Error> {
        self.get_pending_sends().await
    }

    #[instrument(skip(self))]
    async fn revoke_send(&self, operation_id: Uuid) -> Result<Amount, Self::Error> {
        self.revoke_send(operation_id).await
    }

    #[instrument(skip(self))]
    async fn check_send_status(&self, operation_id: Uuid) -> Result<bool, Self::Error> {
        self.check_send_status(operation_id).await
    }

    #[instrument(skip(self, spending_conditions))]
    async fn mint(
        &self,
        quote_id: &str,
        split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Self::Error> {
        self.mint(quote_id, split_target, spending_conditions).await
    }

    #[instrument(skip(self))]
    async fn check_mint_quote_status(&self, quote_id: &str) -> Result<MintQuote, Self::Error> {
        self.check_mint_quote_status(quote_id).await
    }

    #[instrument(skip(self))]
    async fn fetch_mint_quote(
        &self,
        quote_id: &str,
        payment_method: Option<PaymentMethod>,
    ) -> Result<MintQuote, Self::Error> {
        self.fetch_mint_quote(quote_id, payment_method).await
    }

    #[instrument(skip(self, metadata))]
    async fn prepare_melt(
        &self,
        quote_id: &str,
        metadata: HashMap<String, String>,
    ) -> Result<super::melt::PreparedMelt<'_>, Self::Error> {
        self.prepare_melt(quote_id, metadata).await
    }

    #[instrument(skip(self, proofs, metadata))]
    async fn prepare_melt_proofs(
        &self,
        quote_id: &str,
        proofs: Proofs,
        metadata: HashMap<String, String>,
    ) -> Result<super::melt::PreparedMelt<'_>, Self::Error> {
        self.prepare_melt_proofs(quote_id, proofs, metadata).await
    }

    #[instrument(skip(self, input_proofs, spending_conditions))]
    async fn swap(
        &self,
        amount: Option<Amount>,
        split_target: SplitTarget,
        input_proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
        use_p2bk: bool,
    ) -> Result<Option<Proofs>, Self::Error> {
        self.swap(
            amount,
            split_target,
            input_proofs,
            spending_conditions,
            include_fees,
            use_p2bk,
        )
        .await
    }

    #[instrument(skip(self, cat))]
    async fn set_cat(&self, cat: String) -> Result<(), Self::Error> {
        self.set_cat(cat).await
    }

    #[instrument(skip(self, refresh_token))]
    async fn set_refresh_token(&self, refresh_token: String) -> Result<(), Self::Error> {
        self.set_refresh_token(refresh_token).await
    }

    #[instrument(skip(self))]
    async fn refresh_access_token(&self) -> Result<(), Self::Error> {
        self.refresh_access_token().await
    }

    #[instrument(skip(self))]
    async fn mint_blind_auth(&self, amount: Amount) -> Result<Proofs, Self::Error> {
        self.mint_blind_auth(amount).await
    }

    #[instrument(skip(self))]
    async fn get_unspent_auth_proofs(&self) -> Result<Vec<AuthProof>, Self::Error> {
        self.get_unspent_auth_proofs().await
    }

    #[instrument(skip(self))]
    async fn restore(&self) -> Result<Restored, Self::Error> {
        self.restore().await
    }

    #[instrument(skip(self, token_str))]
    async fn verify_token_dleq(&self, token_str: &str) -> Result<(), Self::Error> {
        let token = cdk_common::nuts::nut00::token::Token::from_str(token_str)?;
        self.verify_token_dleq(&token).await
    }

    #[instrument(skip(self, request))]
    async fn pay_request(
        &self,
        request: PaymentRequest,
        custom_amount: Option<Amount>,
    ) -> Result<(), Self::Error> {
        self.pay_request(request, custom_amount).await
    }

    #[instrument(skip(self, method))]
    async fn subscribe_mint_quote_state(
        &self,
        quote_ids: Vec<String>,
        method: PaymentMethod,
    ) -> Result<ActiveSubscription, Self::Error> {
        self.subscribe_mint_quote_state(quote_ids, method).await
    }

    fn set_metadata_cache_ttl(&self, ttl_secs: Option<u64>) {
        let ttl = ttl_secs.map(std::time::Duration::from_secs);
        self.set_metadata_cache_ttl(ttl);
    }

    #[instrument(skip(self, params))]
    async fn subscribe(&self, params: WalletParams) -> Result<ActiveSubscription, Self::Error> {
        self.subscribe(params).await
    }

    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    #[instrument(skip(self, amount_msat), fields(address = %bip353_address))]
    async fn melt_bip353_quote(
        &self,
        bip353_address: &str,
        amount_msat: Amount,
        network: bitcoin::Network,
    ) -> Result<MeltQuote, Self::Error> {
        self.melt_bip353_quote(bip353_address, amount_msat, network)
            .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[instrument(skip(self, amount_msat), fields(address = %lightning_address))]
    async fn melt_lightning_address_quote(
        &self,
        lightning_address: &str,
        amount_msat: Amount,
    ) -> Result<MeltQuote, Self::Error> {
        self.melt_lightning_address_quote(lightning_address, amount_msat)
            .await
    }

    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    #[instrument(skip(self, amount_msat), fields(address = %address))]
    async fn melt_human_readable_quote(
        &self,
        address: &str,
        amount_msat: Amount,
        network: bitcoin::Network,
    ) -> Result<MeltQuote, Self::Error> {
        self.melt_human_readable_quote(address, amount_msat, network)
            .await
    }

    #[instrument(skip(self))]
    async fn get_proofs_by_states(
        &self,
        states: Vec<cdk_common::nuts::State>,
    ) -> Result<Proofs, Self::Error> {
        self.get_proofs_by_states(states).await
    }
}
