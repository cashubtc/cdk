//! Wallet trait implementations for [`Wallet`].
//!
//! Implements the unified wallet trait defined in `cdk_common::wallet`.

use std::str::FromStr;

use cdk_common::nut00::KnownMethod;
use cdk_common::nut04::MintMethodOptions;
use cdk_common::wallet::{Transaction, TransactionDirection, TransactionId, Wallet as WalletTrait};
use cdk_common::CurrencyUnit;

use crate::amount::SplitTarget;
use crate::mint_url::MintUrl;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    Id, KeySetInfo, MeltOptions, MintInfo, MintQuoteBolt11Request, MintQuoteBolt12Request,
    MintQuoteCustomRequest, PaymentMethod, PaymentRequest, Proof, Proofs, SecretKey,
    SpendingConditions, State, Token,
};
use crate::types::FinalizedMelt;
use crate::util::unix_time;
use crate::wallet::receive::ReceiveOptions;
use crate::wallet::send::SendOptions;
use crate::wallet::subscription::ActiveSubscription;
use crate::wallet::{MeltQuote, MintQuote, Restored, Wallet};
use crate::{Amount, Error, OidcClient};

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl WalletTrait for Wallet {
    type Amount = Amount;
    type Proofs = Proofs;
    type Proof = Proof;
    type MintQuote = MintQuote;
    type MeltQuote = MeltQuote;
    type MeltResult = FinalizedMelt;
    type Token = Token;
    type CurrencyUnit = CurrencyUnit;
    type MintUrl = MintUrl;
    type MintInfo = MintInfo;
    type KeySetInfo = KeySetInfo;
    type Error = Error;
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
    type PaymentRequest = PaymentRequest;
    type Subscription = ActiveSubscription;
    type SubscribeParams = cdk_common::subscription::WalletParams;

    fn mint_url(&self) -> Self::MintUrl {
        self.mint_url.clone()
    }

    fn unit(&self) -> Self::CurrencyUnit {
        self.unit.clone()
    }

    async fn total_balance(&self) -> Result<Amount, Error> {
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

    async fn total_pending_balance(&self) -> Result<Amount, Error> {
        Ok(self.get_pending_proofs().await?.total_amount()?)
    }

    async fn total_reserved_balance(&self) -> Result<Amount, Error> {
        Ok(self.get_reserved_proofs().await?.total_amount()?)
    }

    async fn fetch_mint_info(&self) -> Result<Option<MintInfo>, Error> {
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
            use crate::wallet::auth::AuthWallet;

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

    async fn load_mint_info(&self) -> Result<MintInfo, Error> {
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

    async fn get_active_keyset(&self) -> Result<KeySetInfo, Error> {
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

    async fn refresh_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        tracing::debug!("Refreshing keysets from mint");

        let keysets = self
            .metadata_cache
            .load_from_mint(&self.localstore, &self.client)
            .await?
            .keysets
            .iter()
            .filter_map(|(_, keyset)| {
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

    async fn mint_quote(
        &self,
        method: PaymentMethod,
        amount: Option<Amount>,
        description: Option<String>,
        extra: Option<String>,
    ) -> Result<MintQuote, Error> {
        let mint_info = WalletTrait::load_mint_info(self).await?;
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
                Some(MintMethodOptions::Bolt11 {
                    description: true, ..
                }) => (),
                _ => return Err(Error::InvoiceDescriptionUnsupported),
            }
        }

        WalletTrait::refresh_keysets(self).await?;

        let secret_key = SecretKey::generate();

        match method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                let bolt11_amount =
                    amount.ok_or(Error::Custom("Amount is required for Bolt11".to_string()))?;

                let request = MintQuoteBolt11Request {
                    amount: bolt11_amount,
                    unit: unit.clone(),
                    description,
                    pubkey: Some(secret_key.public_key()),
                };

                let response = self.client.post_mint_quote(request).await?;

                let quote = MintQuote::new(
                    response.quote,
                    mint_url,
                    method,
                    Some(bolt11_amount),
                    unit,
                    response.request,
                    response.expiry.unwrap_or(0),
                    Some(secret_key),
                );

                self.localstore.add_mint_quote(quote.clone()).await?;
                Ok(quote)
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                let request = MintQuoteBolt12Request {
                    amount,
                    unit: unit.clone(),
                    description,
                    pubkey: secret_key.public_key(),
                };

                let response = self.client.post_mint_bolt12_quote(request).await?;

                let quote = MintQuote::new(
                    response.quote,
                    mint_url,
                    method,
                    response.amount,
                    unit,
                    response.request,
                    response.expiry.unwrap_or(0),
                    Some(secret_key),
                );

                self.localstore.add_mint_quote(quote.clone()).await?;
                Ok(quote)
            }
            PaymentMethod::Custom(_) => {
                let custom_amount =
                    amount.ok_or(Error::Custom("Amount is required for Custom".to_string()))?;

                let extra_json = extra
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or(serde_json::Value::Null);

                let request = MintQuoteCustomRequest {
                    amount: custom_amount,
                    unit: unit.clone(),
                    description,
                    pubkey: Some(secret_key.public_key()),
                    extra: extra_json,
                };

                let response = self.client.post_mint_custom_quote(&method, request).await?;

                let quote = MintQuote::new(
                    response.quote,
                    mint_url,
                    method,
                    response.amount,
                    unit,
                    response.request,
                    response.expiry.unwrap_or(0),
                    Some(secret_key),
                );

                self.localstore.add_mint_quote(quote.clone()).await?;
                Ok(quote)
            }
        }
    }

    async fn refresh_mint_quote(&self, quote_id: &str) -> Result<MintQuote, Error> {
        Wallet::refresh_mint_quote_status(self, quote_id).await
    }

    async fn melt_quote(
        &self,
        method: PaymentMethod,
        request: String,
        options: Option<MeltOptions>,
        extra: Option<String>,
    ) -> Result<MeltQuote, Error> {
        Wallet::melt_quote::<PaymentMethod, _>(self, method, request, options, extra).await
    }

    async fn send(&self, amount: Amount, options: SendOptions) -> Result<Token, Error> {
        let memo = options.memo.clone();
        let prepared = self.prepare_send(amount, options).await?;
        prepared.confirm(memo).await
    }

    async fn get_pending_sends(&self) -> Result<Vec<String>, Error> {
        let uuids = Wallet::get_pending_sends(self).await?;
        Ok(uuids.into_iter().map(|id| id.to_string()).collect())
    }

    async fn revoke_send(&self, operation_id: &str) -> Result<Amount, Error> {
        let uuid = uuid::Uuid::parse_str(operation_id)
            .map_err(|e| Error::Custom(format!("Invalid operation ID: {}", e)))?;
        Wallet::revoke_send(self, uuid).await
    }

    async fn check_send_status(&self, operation_id: &str) -> Result<bool, Error> {
        let uuid = uuid::Uuid::parse_str(operation_id)
            .map_err(|e| Error::Custom(format!("Invalid operation ID: {}", e)))?;
        Wallet::check_send_status(self, uuid).await
    }

    async fn receive(&self, encoded_token: &str, options: ReceiveOptions) -> Result<Amount, Error> {
        Wallet::receive(self, encoded_token, options).await
    }

    async fn receive_proofs(
        &self,
        proofs: Proofs,
        options: ReceiveOptions,
        memo: Option<String>,
        token: Option<String>,
    ) -> Result<Amount, Error> {
        Wallet::receive_proofs(self, proofs, options, memo, token).await
    }

    async fn swap(
        &self,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        input_proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
    ) -> Result<Option<Proofs>, Error> {
        Wallet::swap(
            self,
            amount,
            amount_split_target,
            input_proofs,
            spending_conditions,
            include_fees,
        )
        .await
    }

    async fn get_unspent_proofs(&self) -> Result<Proofs, Error> {
        Wallet::get_unspent_proofs(self).await
    }

    async fn get_pending_proofs(&self) -> Result<Proofs, Error> {
        Wallet::get_pending_proofs(self).await
    }

    async fn get_reserved_proofs(&self) -> Result<Proofs, Error> {
        Wallet::get_reserved_proofs(self).await
    }

    async fn get_pending_spent_proofs(&self) -> Result<Proofs, Error> {
        Wallet::get_pending_spent_proofs(self).await
    }

    async fn check_all_pending_proofs(&self) -> Result<Amount, Error> {
        Wallet::check_all_pending_proofs(self).await
    }

    async fn check_proofs_spent(&self, proofs: Proofs) -> Result<Vec<bool>, Error> {
        let states = Wallet::check_proofs_spent(self, proofs).await?;
        Ok(states
            .into_iter()
            .map(|ps| matches!(ps.state, State::Spent | State::PendingSpent))
            .collect())
    }

    async fn reclaim_unspent(&self, proofs: Proofs) -> Result<(), Error> {
        let states = Wallet::check_proofs_spent(self, proofs.clone()).await?;
        let unspent_proofs: Proofs = proofs
            .into_iter()
            .zip(states.iter())
            .filter(|(_, ps)| !matches!(ps.state, State::Spent | State::PendingSpent))
            .map(|(p, _)| p)
            .collect();

        if !unspent_proofs.is_empty() {
            self.swap(None, SplitTarget::default(), unspent_proofs, None, false)
                .await?;
        }
        Ok(())
    }

    async fn list_transactions(
        &self,
        direction: Option<TransactionDirection>,
    ) -> Result<Vec<Transaction>, Error> {
        Wallet::list_transactions(self, direction).await
    }

    async fn get_transaction(&self, id: TransactionId) -> Result<Option<Transaction>, Error> {
        Wallet::get_transaction(self, id).await
    }

    async fn get_proofs_for_transaction(&self, id: TransactionId) -> Result<Proofs, Error> {
        Wallet::get_proofs_for_transaction(self, id).await
    }

    async fn revert_transaction(&self, id: TransactionId) -> Result<(), Error> {
        Wallet::revert_transaction(self, id).await
    }

    async fn verify_token_dleq(&self, token: &Token) -> Result<(), Error> {
        Wallet::verify_token_dleq(self, token).await
    }

    async fn restore(&self) -> Result<Restored, Error> {
        Wallet::restore(self).await
    }

    async fn get_keyset_fees(&self, keyset_id: &str) -> Result<u64, Error> {
        let id = Id::from_str(keyset_id)?;
        Ok(self.get_keyset_fees_and_amounts_by_id(id).await?.fee())
    }

    async fn calculate_fee(&self, proof_count: u64, keyset_id: &str) -> Result<Amount, Error> {
        let id = Id::from_str(keyset_id)?;
        Wallet::get_keyset_count_fee(self, &id, proof_count).await
    }

    async fn subscribe(
        &self,
        params: cdk_common::subscription::WalletParams,
    ) -> Result<ActiveSubscription, Error> {
        Wallet::subscribe(self, params).await
    }

    async fn pay_request(
        &self,
        request: PaymentRequest,
        custom_amount: Option<Amount>,
    ) -> Result<(), Error> {
        Wallet::pay_request(self, request, custom_amount).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn melt_bip353_quote(&self, address: &str, amount: Amount) -> Result<MeltQuote, Error> {
        Wallet::melt_bip353_quote(self, address, amount).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn melt_lightning_address_quote(
        &self,
        address: &str,
        amount: Amount,
    ) -> Result<MeltQuote, Error> {
        Wallet::melt_lightning_address_quote(self, address, amount).await
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn melt_human_readable_quote(
        &self,
        address: &str,
        amount: Amount,
    ) -> Result<MeltQuote, Error> {
        Wallet::melt_human_readable_quote(self, address, amount).await
    }

    async fn set_cat(&self, cat: String) -> Result<(), Error> {
        Wallet::set_cat(self, cat).await
    }

    async fn set_refresh_token(&self, refresh_token: String) -> Result<(), Error> {
        Wallet::set_refresh_token(self, refresh_token).await
    }

    async fn refresh_access_token(&self) -> Result<(), Error> {
        Wallet::refresh_access_token(self).await
    }

    async fn mint_blind_auth(&self, amount: Amount) -> Result<Proofs, Error> {
        Wallet::mint_blind_auth(self, amount).await
    }

    async fn get_unspent_auth_proofs(&self) -> Result<Proofs, Error> {
        let auth_proofs = Wallet::get_unspent_auth_proofs(self).await?;
        Ok(auth_proofs.into_iter().map(|ap| ap.into()).collect())
    }
}
