//! WalletTrait implementation for the CDK Wallet.

use std::collections::HashMap;
use std::str::FromStr;

use async_trait::async_trait;
use cdk_common::amount::SplitTarget;
use cdk_common::mint_url::MintUrl;
use cdk_common::nut00::KnownMethod;
use cdk_common::nut04::MintMethodOptions;
use cdk_common::nuts::nut07::ProofState;
use cdk_common::nuts::nut18::PaymentRequest;
use cdk_common::nuts::{
    AuthProof, CurrencyUnit, Id, KeySetInfo, MeltOptions, MintInfo, PaymentMethod, Proofs,
    SecretKey, SpendingConditions, State,
};
use cdk_common::wallet::{
    MeltQuote, MintQuote, PreparedMeltData, PreparedSendData, ReceiveOptions, Restored,
    SendOptions, Transaction, TransactionDirection, TransactionId, Wallet as WalletTrait,
};
use cdk_common::{Amount, MintQuoteRequest, MintQuoteResponse};
use tracing::instrument;
use uuid::Uuid;

use super::AuthWallet;
use crate::nuts::nut00::ProofsMethods;
use crate::util::unix_time;
use crate::wallet::subscription::ActiveSubscription;
use crate::wallet::WalletSubscription;
use crate::{Error, OidcClient};

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
    type PreparedSend = PreparedSendData;
    type PreparedMelt = PreparedMeltData;
    type Subscription = ActiveSubscription;

    fn mint_url(&self) -> MintUrl {
        self.mint_url.clone()
    }

    fn unit(&self) -> CurrencyUnit {
        self.unit.clone()
    }

    // === Balance (moved from balance.rs) ===

    #[instrument(skip(self))]
    async fn total_balance(&self) -> Result<Amount, Self::Error> {
        let balance = self
            .localstore
            .get_balance(
                Some(self.mint_url.clone()),
                Some(self.unit.clone()),
                Some(vec![State::Unspent]),
            )
            .await?;
        Ok(Amount::from(balance))
    }

    #[instrument(skip(self))]
    async fn total_pending_balance(&self) -> Result<Amount, Self::Error> {
        Ok(self.get_pending_proofs().await?.total_amount()?)
    }

    #[instrument(skip(self))]
    async fn total_reserved_balance(&self) -> Result<Amount, Self::Error> {
        Ok(self.get_reserved_proofs().await?.total_amount()?)
    }

    // === Mint Info (moved from mod.rs) ===

    #[instrument(skip(self))]
    async fn fetch_mint_info(&self) -> Result<Option<MintInfo>, Self::Error> {
        let mint_info = self
            .metadata_cache
            .load_from_mint(&self.localstore, &self.client)
            .await?
            .mint_info
            .clone();

        // If mint provides time make sure it is accurate
        if let Some(mint_unix_time) = mint_info.time {
            let current_unix_time = unix_time();
            if current_unix_time.abs_diff(mint_unix_time) > 30 {
                tracing::warn!(
                    "Mint time does match wallet time. Mint: {}, Wallet: {}",
                    mint_unix_time,
                    current_unix_time
                );
                return Err(Error::MintTimeExceedsTolerance);
            }
        }

        // Create or update auth wallet
        {
            let mut auth_wallet = self.auth_wallet.write().await;
            match &*auth_wallet {
                Some(auth_wallet) => {
                    let mut protected_endpoints = auth_wallet.protected_endpoints.write().await;
                    *protected_endpoints = mint_info.protected_endpoints();

                    if let Some(oidc_client) = mint_info
                        .openid_discovery()
                        .map(|url| OidcClient::new(url, None))
                    {
                        auth_wallet.set_oidc_client(Some(oidc_client)).await;
                    }
                }
                None => {
                    tracing::info!("Mint has auth enabled creating auth wallet");

                    let oidc_client = mint_info
                        .openid_discovery()
                        .map(|url| OidcClient::new(url, None));
                    let new_auth_wallet = AuthWallet::new(
                        self.mint_url.clone(),
                        None,
                        self.localstore.clone(),
                        self.metadata_cache.clone(),
                        mint_info.protected_endpoints(),
                        oidc_client,
                    );
                    *auth_wallet = Some(new_auth_wallet.clone());

                    self.client
                        .set_auth_wallet(Some(new_auth_wallet.clone()))
                        .await;

                    if let Err(e) = new_auth_wallet.refresh_keysets().await {
                        tracing::error!("Could not fetch auth keysets: {}", e);
                    }
                }
            }
        }

        tracing::trace!("Mint info updated for {}", self.mint_url);

        Ok(Some(mint_info))
    }

    #[instrument(skip(self))]
    async fn load_mint_info(&self) -> Result<MintInfo, Self::Error> {
        let mint_info = self
            .metadata_cache
            .load(&self.localstore, &self.client, {
                let ttl = self.metadata_cache_ttl.read();
                *ttl
            })
            .await?
            .mint_info
            .clone();

        Ok(mint_info)
    }

    // === Keysets (moved from keysets.rs) ===

    #[instrument(skip(self))]
    async fn refresh_keysets(&self) -> Result<Vec<KeySetInfo>, Self::Error> {
        tracing::debug!("Refreshing keysets from mint");

        let keysets = self
            .metadata_cache
            .load_from_mint(&self.localstore, &self.client)
            .await?
            .keysets
            .values()
            .filter_map(|keyset| {
                if keyset.unit == self.unit && keyset.active {
                    Some((*keyset.clone()).clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        if !keysets.is_empty() {
            Ok(keysets)
        } else {
            Err(Error::UnknownKeySet)
        }
    }

    #[instrument(skip(self))]
    async fn get_active_keyset(&self) -> Result<KeySetInfo, Self::Error> {
        self.metadata_cache
            .load(&self.localstore, &self.client, {
                let ttl = self.metadata_cache_ttl.read();
                *ttl
            })
            .await?
            .active_keysets
            .iter()
            .min_by_key(|k| k.input_fee_ppk)
            .map(|ks| (**ks).clone())
            .ok_or(Error::NoActiveKeyset)
    }

    // === Minting (moved from issue/mod.rs) ===

    #[instrument(skip(self, method))]
    async fn mint_quote(
        &self,
        method: PaymentMethod,
        amount: Option<Amount>,
        description: Option<String>,
        extra: Option<String>,
    ) -> Result<MintQuote, Self::Error> {
        let mint_info = self.load_mint_info().await?;
        let mint_url = self.mint_url.clone();
        let unit = self.unit.clone();

        // Check settings and description support
        if description.is_some() {
            let settings = mint_info
                .nuts
                .nut04
                .get_settings(&unit, &method)
                .ok_or(Error::UnsupportedUnit)?;

            match settings.options {
                Some(MintMethodOptions::Bolt11 { description }) if description => (),
                _ => return Err(Error::InvoiceDescriptionUnsupported),
            }
        }

        self.refresh_keysets().await?;

        let secret_key = SecretKey::generate();

        let request = match &method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                let amount = amount.ok_or(Error::AmountUndefined)?;
                MintQuoteRequest::Bolt11(cdk_common::nut23::MintQuoteBolt11Request {
                    amount,
                    unit: unit.clone(),
                    description,
                    pubkey: Some(secret_key.public_key()),
                })
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                MintQuoteRequest::Bolt12(cdk_common::nut25::MintQuoteBolt12Request {
                    amount,
                    unit: unit.clone(),
                    description,
                    pubkey: secret_key.public_key(),
                })
            }
            PaymentMethod::Custom(_) => {
                let amount = amount.ok_or(Error::AmountUndefined)?;
                MintQuoteRequest::Custom((
                    method.clone(),
                    cdk_common::nuts::MintQuoteCustomRequest {
                        amount,
                        unit: unit.clone(),
                        description,
                        pubkey: Some(secret_key.public_key()),
                        extra: serde_json::from_str(&extra.unwrap_or_default())?,
                    },
                ))
            }
        };

        let response: MintQuoteResponse<String> = self.client.post_mint_quote(request).await?;
        let quote_id = response.quote().to_string();
        let request_str = response.request().to_string();
        let expiry = response.expiry();

        let quote = MintQuote::new(
            quote_id,
            mint_url,
            method.clone(),
            amount,
            unit,
            request_str,
            expiry.unwrap_or(0),
            Some(secret_key),
        );

        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote)
    }

    // === Melting (delegates to inherent methods) ===

    #[instrument(skip(self, request, options, extra))]
    async fn melt_quote(
        &self,
        method: PaymentMethod,
        request: String,
        options: Option<MeltOptions>,
        extra: Option<String>,
    ) -> Result<MeltQuote, Self::Error> {
        match method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                self.melt_bolt11_quote(request, options).await
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                self.melt_bolt12_quote(request, options).await
            }
            PaymentMethod::Custom(custom_method) => {
                let extra_json =
                    extra.map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null));
                self.melt_quote_custom(&custom_method, request, options, extra_json)
                    .await
            }
        }
    }

    // === Transactions ===

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

    // === Proofs ===

    #[instrument(skip(self))]
    async fn check_all_pending_proofs(&self) -> Result<Amount, Self::Error> {
        self.check_all_pending_proofs().await
    }

    #[instrument(skip(self, proofs))]
    async fn check_proofs_spent(&self, proofs: Proofs) -> Result<Vec<ProofState>, Self::Error> {
        self.check_proofs_spent(proofs).await
    }

    // === Fees ===

    #[instrument(skip(self))]
    async fn get_keyset_fees_by_id(&self, keyset_id: Id) -> Result<u64, Self::Error> {
        Ok(self
            .get_keyset_fees_and_amounts_by_id(keyset_id)
            .await?
            .fee())
    }

    #[instrument(skip(self))]
    async fn calculate_fee(&self, proof_count: u64, keyset_id: Id) -> Result<Amount, Self::Error> {
        self.get_keyset_count_fee(&keyset_id, proof_count).await
    }

    // === Receive ===

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

    // === Send ===

    #[instrument(skip(self, options))]
    async fn prepare_send(
        &self,
        amount: Amount,
        options: SendOptions,
    ) -> Result<PreparedSendData, Self::Error> {
        let prepared = self.prepare_send(amount, options).await?;
        Ok(PreparedSendData {
            operation_id: prepared.operation_id(),
            amount: prepared.amount(),
            options: prepared.options().clone(),
            proofs_to_swap: prepared.proofs_to_swap().clone(),
            proofs_to_send: prepared.proofs_to_send().clone(),
            swap_fee: prepared.swap_fee(),
            send_fee: prepared.send_fee(),
        })
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

    // === Mint (Issue) ===

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

    // === Melt ===

    #[instrument(skip(self, metadata))]
    async fn prepare_melt(
        &self,
        quote_id: &str,
        metadata: HashMap<String, String>,
    ) -> Result<PreparedMeltData, Self::Error> {
        let prepared = self.prepare_melt(quote_id, metadata.clone()).await?;
        Ok(PreparedMeltData {
            operation_id: prepared.operation_id(),
            quote: prepared.quote().clone(),
            proofs: prepared.proofs().clone(),
            proofs_to_swap: prepared.proofs_to_swap().clone(),
            swap_fee: prepared.swap_fee(),
            input_fee: prepared.input_fee(),
            input_fee_without_swap: prepared.input_fee_without_swap(),
            metadata,
        })
    }

    #[instrument(skip(self, proofs, metadata))]
    async fn prepare_melt_proofs(
        &self,
        quote_id: &str,
        proofs: Proofs,
        metadata: HashMap<String, String>,
    ) -> Result<PreparedMeltData, Self::Error> {
        let prepared = self
            .prepare_melt_proofs(quote_id, proofs, metadata.clone())
            .await?;
        Ok(PreparedMeltData {
            operation_id: prepared.operation_id(),
            quote: prepared.quote().clone(),
            proofs: prepared.proofs().clone(),
            proofs_to_swap: prepared.proofs_to_swap().clone(),
            swap_fee: prepared.swap_fee(),
            input_fee: prepared.input_fee(),
            input_fee_without_swap: prepared.input_fee_without_swap(),
            metadata,
        })
    }

    // === Swap ===

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

    // === Auth ===

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

    // === Restore ===

    #[instrument(skip(self))]
    async fn restore(&self) -> Result<Restored, Self::Error> {
        self.restore().await
    }

    // === Verification ===

    #[instrument(skip(self, token_str))]
    async fn verify_token_dleq(&self, token_str: &str) -> Result<(), Self::Error> {
        let token = cdk_common::nuts::nut00::token::Token::from_str(token_str)?;
        self.verify_token_dleq(&token).await
    }

    // === Payment Requests ===

    #[instrument(skip(self, request))]
    async fn pay_request(
        &self,
        request: PaymentRequest,
        custom_amount: Option<Amount>,
    ) -> Result<(), Self::Error> {
        self.pay_request(request, custom_amount).await
    }

    // === Subscriptions ===

    #[instrument(skip(self, method))]
    async fn subscribe_mint_quote_state(
        &self,
        quote_ids: Vec<String>,
        method: PaymentMethod,
    ) -> Result<ActiveSubscription, Self::Error> {
        let sub = match method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                WalletSubscription::Bolt11MintQuoteState(quote_ids)
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                WalletSubscription::Bolt12MintQuoteState(quote_ids)
            }
            PaymentMethod::Custom(_) => {
                return Err(Error::InvalidPaymentMethod);
            }
        };
        self.subscribe(sub).await
    }
}
