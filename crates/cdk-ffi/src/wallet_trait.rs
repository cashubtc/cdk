//! Wallet trait implementations for the FFI [`Wallet`].
//!
//! Each method delegates to the CDK wallet's trait implementation,
//! converting types between FFI and CDK representations.

use std::sync::Arc;

use cdk_common::wallet::Wallet as CdkWalletTrait;

use crate::error::FfiError;
use crate::types::*;
use crate::wallet::Wallet;

#[async_trait::async_trait]
impl CdkWalletTrait for Wallet {
    type Amount = Amount;
    type Proofs = Proofs;
    type Proof = Proof;
    type MintQuote = MintQuote;
    type MeltQuote = MeltQuote;
    type MeltResult = FinalizedMelt;
    type Token = String;
    type CurrencyUnit = CurrencyUnit;
    type MintUrl = MintUrl;
    type MintInfo = MintInfo;
    type KeySetInfo = KeySetInfo;
    type Error = FfiError;
    type SendOptions = SendOptions;
    type ReceiveOptions = ReceiveOptions;
    type SpendingConditions = SpendingConditions;
    type SplitTarget = SplitTarget;
    type PaymentMethod = PaymentMethod;
    type MeltOptions = MeltOptions;
    type Restored = Restored;
    type Transaction = Transaction;
    type TransactionId = TransactionId;
    type TransactionDirection = TransactionDirection;
    type PaymentRequest = Arc<crate::types::payment_request::PaymentRequest>;
    type Subscription = Arc<ActiveSubscription>;
    type SubscribeParams = SubscribeParams;

    fn mint_url(&self) -> Self::MintUrl {
        <cdk::wallet::Wallet as CdkWalletTrait>::mint_url(self.inner().as_ref()).into()
    }

    fn unit(&self) -> Self::CurrencyUnit {
        <cdk::wallet::Wallet as CdkWalletTrait>::unit(self.inner().as_ref()).into()
    }

    async fn total_balance(&self) -> Result<Self::Amount, Self::Error> {
        Ok(
            <cdk::wallet::Wallet as CdkWalletTrait>::total_balance(self.inner().as_ref())
                .await?
                .into(),
        )
    }

    async fn total_pending_balance(&self) -> Result<Self::Amount, Self::Error> {
        Ok(
            <cdk::wallet::Wallet as CdkWalletTrait>::total_pending_balance(self.inner().as_ref())
                .await?
                .into(),
        )
    }

    async fn total_reserved_balance(&self) -> Result<Self::Amount, Self::Error> {
        Ok(
            <cdk::wallet::Wallet as CdkWalletTrait>::total_reserved_balance(self.inner().as_ref())
                .await?
                .into(),
        )
    }

    async fn fetch_mint_info(&self) -> Result<Option<Self::MintInfo>, Self::Error> {
        let info =
            <cdk::wallet::Wallet as CdkWalletTrait>::fetch_mint_info(self.inner().as_ref()).await?;
        Ok(info.map(Into::into))
    }

    async fn load_mint_info(&self) -> Result<Self::MintInfo, Self::Error> {
        Ok(
            <cdk::wallet::Wallet as CdkWalletTrait>::load_mint_info(self.inner().as_ref())
                .await?
                .into(),
        )
    }

    async fn get_active_keyset(&self) -> Result<Self::KeySetInfo, Self::Error> {
        Ok(
            <cdk::wallet::Wallet as CdkWalletTrait>::get_active_keyset(self.inner().as_ref())
                .await?
                .into(),
        )
    }

    async fn refresh_keysets(&self) -> Result<Vec<Self::KeySetInfo>, Self::Error> {
        let keysets =
            <cdk::wallet::Wallet as CdkWalletTrait>::refresh_keysets(self.inner().as_ref()).await?;
        Ok(keysets.into_iter().map(Into::into).collect())
    }

    async fn mint_quote(
        &self,
        method: Self::PaymentMethod,
        amount: Option<Self::Amount>,
        description: Option<String>,
        extra: Option<String>,
    ) -> Result<Self::MintQuote, Self::Error> {
        Ok(<cdk::wallet::Wallet as CdkWalletTrait>::mint_quote(
            self.inner().as_ref(),
            method.into(),
            amount.map(Into::into),
            description,
            extra,
        )
        .await?
        .into())
    }

    async fn refresh_mint_quote(&self, quote_id: &str) -> Result<Self::MintQuote, Self::Error> {
        Ok(<cdk::wallet::Wallet as CdkWalletTrait>::refresh_mint_quote(
            self.inner().as_ref(),
            quote_id,
        )
        .await?
        .into())
    }

    async fn melt_quote(
        &self,
        method: Self::PaymentMethod,
        request: String,
        options: Option<Self::MeltOptions>,
        extra: Option<String>,
    ) -> Result<Self::MeltQuote, Self::Error> {
        Ok(<cdk::wallet::Wallet as CdkWalletTrait>::melt_quote(
            self.inner().as_ref(),
            method.into(),
            request,
            options.map(Into::into),
            extra,
        )
        .await?
        .into())
    }

    async fn send(
        &self,
        amount: Self::Amount,
        options: Self::SendOptions,
    ) -> Result<Self::Token, Self::Error> {
        let token = <cdk::wallet::Wallet as CdkWalletTrait>::send(
            self.inner().as_ref(),
            amount.into(),
            options.into(),
        )
        .await?;
        Ok(token.to_string())
    }

    async fn get_pending_sends(&self) -> Result<Vec<String>, Self::Error> {
        Ok(
            <cdk::wallet::Wallet as CdkWalletTrait>::get_pending_sends(self.inner().as_ref())
                .await?,
        )
    }

    async fn revoke_send(&self, operation_id: &str) -> Result<Self::Amount, Self::Error> {
        Ok(<cdk::wallet::Wallet as CdkWalletTrait>::revoke_send(
            self.inner().as_ref(),
            operation_id,
        )
        .await?
        .into())
    }

    async fn check_send_status(&self, operation_id: &str) -> Result<bool, Self::Error> {
        Ok(<cdk::wallet::Wallet as CdkWalletTrait>::check_send_status(
            self.inner().as_ref(),
            operation_id,
        )
        .await?)
    }

    async fn receive(
        &self,
        encoded_token: &str,
        options: Self::ReceiveOptions,
    ) -> Result<Self::Amount, Self::Error> {
        Ok(<cdk::wallet::Wallet as CdkWalletTrait>::receive(
            self.inner().as_ref(),
            encoded_token,
            options.into(),
        )
        .await?
        .into())
    }

    async fn receive_proofs(
        &self,
        proofs: Self::Proofs,
        options: Self::ReceiveOptions,
        memo: Option<String>,
        token: Option<String>,
    ) -> Result<Self::Amount, Self::Error> {
        let cdk_proofs: Result<Vec<cdk::nuts::Proof>, _> =
            proofs.into_iter().map(|p| p.try_into()).collect();
        let cdk_proofs = cdk_proofs?;

        Ok(<cdk::wallet::Wallet as CdkWalletTrait>::receive_proofs(
            self.inner().as_ref(),
            cdk_proofs,
            options.into(),
            memo,
            token,
        )
        .await?
        .into())
    }

    async fn swap(
        &self,
        amount: Option<Self::Amount>,
        amount_split_target: Self::SplitTarget,
        input_proofs: Self::Proofs,
        spending_conditions: Option<Self::SpendingConditions>,
        include_fees: bool,
    ) -> Result<Option<Self::Proofs>, Self::Error> {
        let cdk_proofs: Result<Vec<cdk::nuts::Proof>, _> =
            input_proofs.into_iter().map(|p| p.try_into()).collect();
        let cdk_proofs = cdk_proofs?;

        let conditions = spending_conditions.map(|sc| sc.try_into()).transpose()?;

        let result = <cdk::wallet::Wallet as CdkWalletTrait>::swap(
            self.inner().as_ref(),
            amount.map(Into::into),
            amount_split_target.into(),
            cdk_proofs,
            conditions,
            include_fees,
        )
        .await?;

        Ok(result.map(|proofs| proofs.into_iter().map(|p| p.into()).collect()))
    }

    async fn get_unspent_proofs(&self) -> Result<Self::Proofs, Self::Error> {
        let proofs =
            <cdk::wallet::Wallet as CdkWalletTrait>::get_unspent_proofs(self.inner().as_ref())
                .await?;
        Ok(proofs.into_iter().map(Into::into).collect())
    }

    async fn get_pending_proofs(&self) -> Result<Self::Proofs, Self::Error> {
        let proofs =
            <cdk::wallet::Wallet as CdkWalletTrait>::get_pending_proofs(self.inner().as_ref())
                .await?;
        Ok(proofs.into_iter().map(Into::into).collect())
    }

    async fn get_reserved_proofs(&self) -> Result<Self::Proofs, Self::Error> {
        let proofs =
            <cdk::wallet::Wallet as CdkWalletTrait>::get_reserved_proofs(self.inner().as_ref())
                .await?;
        Ok(proofs.into_iter().map(Into::into).collect())
    }

    async fn get_pending_spent_proofs(&self) -> Result<Self::Proofs, Self::Error> {
        let proofs = <cdk::wallet::Wallet as CdkWalletTrait>::get_pending_spent_proofs(
            self.inner().as_ref(),
        )
        .await?;
        Ok(proofs.into_iter().map(Into::into).collect())
    }

    async fn check_all_pending_proofs(&self) -> Result<Self::Amount, Self::Error> {
        Ok(
            <cdk::wallet::Wallet as CdkWalletTrait>::check_all_pending_proofs(
                self.inner().as_ref(),
            )
            .await?
            .into(),
        )
    }

    async fn check_proofs_spent(&self, proofs: Self::Proofs) -> Result<Vec<bool>, Self::Error> {
        let cdk_proofs: Result<Vec<cdk::nuts::Proof>, _> =
            proofs.into_iter().map(|p| p.try_into()).collect();
        let cdk_proofs = cdk_proofs?;

        Ok(<cdk::wallet::Wallet as CdkWalletTrait>::check_proofs_spent(
            self.inner().as_ref(),
            cdk_proofs,
        )
        .await?)
    }

    async fn reclaim_unspent(&self, proofs: Self::Proofs) -> Result<(), Self::Error> {
        let cdk_proofs: Result<Vec<cdk::nuts::Proof>, _> =
            proofs.into_iter().map(|p| p.try_into()).collect();
        let cdk_proofs = cdk_proofs?;

        Ok(<cdk::wallet::Wallet as CdkWalletTrait>::reclaim_unspent(
            self.inner().as_ref(),
            cdk_proofs,
        )
        .await?)
    }

    async fn list_transactions(
        &self,
        direction: Option<Self::TransactionDirection>,
    ) -> Result<Vec<Self::Transaction>, Self::Error> {
        let cdk_direction = direction.map(Into::into);
        let transactions = <cdk::wallet::Wallet as CdkWalletTrait>::list_transactions(
            self.inner().as_ref(),
            cdk_direction,
        )
        .await?;
        Ok(transactions.into_iter().map(Into::into).collect())
    }

    async fn get_transaction(
        &self,
        id: Self::TransactionId,
    ) -> Result<Option<Self::Transaction>, Self::Error> {
        let cdk_id = id.try_into()?;
        let transaction =
            <cdk::wallet::Wallet as CdkWalletTrait>::get_transaction(self.inner().as_ref(), cdk_id)
                .await?;
        Ok(transaction.map(Into::into))
    }

    async fn get_proofs_for_transaction(
        &self,
        id: Self::TransactionId,
    ) -> Result<Self::Proofs, Self::Error> {
        let cdk_id = id.try_into()?;
        let proofs = <cdk::wallet::Wallet as CdkWalletTrait>::get_proofs_for_transaction(
            self.inner().as_ref(),
            cdk_id,
        )
        .await?;
        Ok(proofs.into_iter().map(Into::into).collect())
    }

    async fn revert_transaction(&self, id: Self::TransactionId) -> Result<(), Self::Error> {
        let cdk_id = id.try_into()?;
        <cdk::wallet::Wallet as CdkWalletTrait>::revert_transaction(self.inner().as_ref(), cdk_id)
            .await?;
        Ok(())
    }

    async fn verify_token_dleq(&self, token: &String) -> Result<(), Self::Error> {
        let cdk_token: cdk::nuts::Token = token.parse().map_err(FfiError::internal)?;
        <cdk::wallet::Wallet as CdkWalletTrait>::verify_token_dleq(
            self.inner().as_ref(),
            &cdk_token,
        )
        .await?;
        Ok(())
    }

    async fn restore(&self) -> Result<Self::Restored, Self::Error> {
        Ok(
            <cdk::wallet::Wallet as CdkWalletTrait>::restore(self.inner().as_ref())
                .await?
                .into(),
        )
    }

    async fn get_keyset_fees(&self, keyset_id: &str) -> Result<u64, Self::Error> {
        Ok(<cdk::wallet::Wallet as CdkWalletTrait>::get_keyset_fees(
            self.inner().as_ref(),
            keyset_id,
        )
        .await?)
    }

    async fn calculate_fee(
        &self,
        proof_count: u64,
        keyset_id: &str,
    ) -> Result<Self::Amount, Self::Error> {
        Ok(<cdk::wallet::Wallet as CdkWalletTrait>::calculate_fee(
            self.inner().as_ref(),
            proof_count,
            keyset_id,
        )
        .await?
        .into())
    }

    async fn subscribe(
        &self,
        params: Self::SubscribeParams,
    ) -> Result<Self::Subscription, Self::Error> {
        let cdk_params: cdk::nuts::nut17::Params<Arc<String>> = params.clone().into();
        let sub_id = cdk_params.id.to_string();
        let active_sub =
            <cdk::wallet::Wallet as CdkWalletTrait>::subscribe(self.inner().as_ref(), cdk_params)
                .await?;
        Ok(Arc::new(ActiveSubscription::new(active_sub, sub_id)))
    }

    async fn pay_request(
        &self,
        request: Self::PaymentRequest,
        custom_amount: Option<Self::Amount>,
    ) -> Result<(), Self::Error> {
        <cdk::wallet::Wallet as CdkWalletTrait>::pay_request(
            self.inner().as_ref(),
            request.inner().clone(),
            custom_amount.map(Into::into),
        )
        .await?;
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn melt_bip353_quote(
        &self,
        address: &str,
        amount: Self::Amount,
    ) -> Result<Self::MeltQuote, Self::Error> {
        Ok(<cdk::wallet::Wallet as CdkWalletTrait>::melt_bip353_quote(
            self.inner().as_ref(),
            address,
            amount.into(),
        )
        .await?
        .into())
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn melt_lightning_address_quote(
        &self,
        address: &str,
        amount: Self::Amount,
    ) -> Result<Self::MeltQuote, Self::Error> {
        Ok(
            <cdk::wallet::Wallet as CdkWalletTrait>::melt_lightning_address_quote(
                self.inner().as_ref(),
                address,
                amount.into(),
            )
            .await?
            .into(),
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn melt_human_readable_quote(
        &self,
        address: &str,
        amount: Self::Amount,
    ) -> Result<Self::MeltQuote, Self::Error> {
        Ok(
            <cdk::wallet::Wallet as CdkWalletTrait>::melt_human_readable_quote(
                self.inner().as_ref(),
                address,
                amount.into(),
            )
            .await?
            .into(),
        )
    }

    async fn set_cat(&self, cat: String) -> Result<(), Self::Error> {
        <cdk::wallet::Wallet as CdkWalletTrait>::set_cat(self.inner().as_ref(), cat).await?;
        Ok(())
    }

    async fn set_refresh_token(&self, refresh_token: String) -> Result<(), Self::Error> {
        <cdk::wallet::Wallet as CdkWalletTrait>::set_refresh_token(
            self.inner().as_ref(),
            refresh_token,
        )
        .await?;
        Ok(())
    }

    async fn refresh_access_token(&self) -> Result<(), Self::Error> {
        <cdk::wallet::Wallet as CdkWalletTrait>::refresh_access_token(self.inner().as_ref())
            .await?;
        Ok(())
    }

    async fn mint_blind_auth(&self, amount: Self::Amount) -> Result<Self::Proofs, Self::Error> {
        let proofs = <cdk::wallet::Wallet as CdkWalletTrait>::mint_blind_auth(
            self.inner().as_ref(),
            amount.into(),
        )
        .await?;
        Ok(proofs.into_iter().map(Into::into).collect())
    }

    async fn get_unspent_auth_proofs(&self) -> Result<Self::Proofs, Self::Error> {
        let proofs =
            <cdk::wallet::Wallet as CdkWalletTrait>::get_unspent_auth_proofs(self.inner().as_ref())
                .await?;
        Ok(proofs.into_iter().map(Into::into).collect())
    }
}
