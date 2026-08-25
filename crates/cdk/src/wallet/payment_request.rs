//! Utilities for paying NUT-18 Payment Requests.
//!
//! This module prepares and broadcasts payments for Cashu NUT-18 payment requests using either
//! Nostr or HTTP transports when available. If no transport is present in the request, an error
//! is returned so callers can handle alternative delivery mechanisms explicitly.

use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use bitcoin::hashes::sha256::Hash as Sha256Hash;
use cdk_common::{
    Amount, HttpClient, PaymentRequest, PaymentRequestPayload, SupportedMethod, TransportType,
};
#[cfg(feature = "nostr")]
use nostr_sdk::nips::nip19::Nip19Profile;
#[cfg(feature = "nostr")]
use nostr_sdk::prelude::*;
#[cfg(feature = "nostr")]
use nostr_sdk::{Client as NostrClient, EventBuilder, FromBech32, Keys, ToBech32};
use tracing::instrument;

use crate::error::Error;
use crate::mint_url::MintUrl;
use crate::nuts::nut05::MeltMethodSettings;
use crate::nuts::nut10::{Conditions, SpendingConditions};
use crate::nuts::nut11::SigFlag;
use crate::nuts::nut18::Nut10SecretRequest;
use crate::nuts::{CurrencyUnit, Nut10Secret, PaymentMethod, Transport};
#[cfg(feature = "nostr")]
use crate::wallet::ReceiveOptions;
use crate::wallet::{SendOptions, WalletRepository};
use crate::Wallet;

/// Optional limits that callers can check before confirming a prepared NUT-18
/// payment request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PayRequestOptions {
    /// Maximum receiver-selected method fee (`mf`) that may be paid.
    ///
    /// `None` accepts any method fee.
    pub max_method_fee: Option<Amount>,
    /// Maximum total wallet debit, including method and mint input fees.
    pub max_total_amount: Option<Amount>,
}

/// A prepared NUT-18 payment request with its exact fees frozen for review.
///
/// Call [`Self::confirm`] to send and deliver the payment, or [`Self::cancel`]
/// to release the proofs reserved during preparation.
#[must_use = "must be confirmed or canceled to release reserved proofs"]
pub struct PreparedPaymentRequest {
    wallet: Wallet,
    payment_request: PaymentRequest,
    transport: Transport,
    operation_id: uuid::Uuid,
    requested_amount: Amount,
    method: Option<String>,
    method_fee: Amount,
    payment_amount: Amount,
    swap_fee: Amount,
    send_fee: Amount,
    input_fee: Amount,
    total_amount: Amount,
    send_options: SendOptions,
    proofs_to_swap: crate::nuts::Proofs,
    proofs_to_send: crate::nuts::Proofs,
}

impl std::fmt::Debug for PreparedPaymentRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedPaymentRequest")
            .field("operation_id", &self.operation_id)
            .field("mint_url", &self.wallet.mint_url)
            .field("requested_amount", &self.requested_amount)
            .field("method", &self.method)
            .field("method_fee", &self.method_fee)
            .field("swap_fee", &self.swap_fee)
            .field("send_fee", &self.send_fee)
            .field("input_fee", &self.input_fee)
            .field("total_amount", &self.total_amount)
            .finish_non_exhaustive()
    }
}

impl PreparedPaymentRequest {
    /// Operation ID of the reserved send.
    pub fn operation_id(&self) -> uuid::Uuid {
        self.operation_id
    }

    /// Mint selected for the payment.
    pub fn mint_url(&self) -> &MintUrl {
        &self.wallet.mint_url
    }

    /// Currency unit of the payment.
    pub fn unit(&self) -> &CurrencyUnit {
        &self.wallet.unit
    }

    /// Amount requested by the receiver, before the method fee.
    pub fn requested_amount(&self) -> Amount {
        self.requested_amount
    }

    /// Selected payment method, when the request restricts methods.
    pub fn method(&self) -> Option<&str> {
        self.method.as_deref()
    }

    /// Receiver-selected method fee (`mf`) that applies to this mint.
    pub fn method_fee(&self) -> Amount {
        self.method_fee
    }

    /// Requested amount plus the applicable method fee.
    pub fn payment_amount(&self) -> Amount {
        self.payment_amount
    }

    /// Mint input fee paid while swapping proofs into the required denominations.
    pub fn swap_fee(&self) -> Amount {
        self.swap_fee
    }

    /// Mint input fee added so the receiver obtains the full payment amount.
    pub fn send_fee(&self) -> Amount {
        self.send_fee
    }

    /// Total mint input fee for the payment.
    pub fn input_fee(&self) -> Amount {
        self.input_fee
    }

    /// Total amount deducted from the wallet.
    pub fn total_amount(&self) -> Amount {
        self.total_amount
    }

    /// Check this prepared payment against caller-provided fee limits.
    pub fn check_limits(&self, options: PayRequestOptions) -> Result<(), Error> {
        check_payment_request_limits(self.method_fee, self.total_amount, options)
    }

    /// Confirm and deliver the prepared payment.
    ///
    /// If token creation succeeds but transport delivery fails, this returns
    /// [`Error::PaymentRequestDeliveryFailed`]. The token remains a pending
    /// send: do not prepare the payment again. Use the error's operation ID
    /// with [`Wallet::revoke_send`] to reclaim the token if it has not already
    /// been claimed by the receiver.
    #[instrument(skip_all)]
    pub async fn confirm(self) -> Result<(), Error> {
        let operation_id = self.operation_id;
        let token = self
            .wallet
            .confirm_send(
                operation_id,
                self.payment_amount,
                self.send_options,
                self.proofs_to_swap,
                self.proofs_to_send,
                self.swap_fee,
                self.send_fee,
                None,
            )
            .await?;

        let delivery_result = self
            .wallet
            .deliver_payment_request(&self.payment_request, &self.transport, &token)
            .await;

        payment_request_delivery_result(operation_id, delivery_result)
    }

    /// Cancel the prepared payment and release its reserved proofs.
    #[instrument(skip_all)]
    pub async fn cancel(self) -> Result<(), Error> {
        self.wallet
            .cancel_send(self.operation_id, self.proofs_to_swap, self.proofs_to_send)
            .await
    }
}

fn payment_request_delivery_result(
    operation_id: uuid::Uuid,
    delivery_result: Result<(), Error>,
) -> Result<(), Error> {
    delivery_result.map_err(|source| Error::PaymentRequestDeliveryFailed {
        operation_id,
        source: Box::new(source),
    })
}

#[cfg(feature = "nostr")]
fn ensure_nostr_delivery_succeeded(gift_wrap: &Output<EventId>) -> Result<(), Error> {
    if gift_wrap.success.is_empty() {
        let mut failed_relays = gift_wrap
            .failed
            .keys()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        failed_relays.sort();

        return Err(Error::NostrPublishFailed {
            event_id: gift_wrap.val.to_string(),
            failed_relays,
        });
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectedPaymentMethod {
    method: Option<String>,
    fee: Amount,
}

fn check_payment_request_limits(
    method_fee: Amount,
    total_amount: Amount,
    options: PayRequestOptions,
) -> Result<(), Error> {
    if options
        .max_method_fee
        .is_some_and(|max_fee| method_fee > max_fee)
        || options
            .max_total_amount
            .is_some_and(|max_amount| total_amount > max_amount)
    {
        return Err(Error::MaxFeeExceeded);
    }

    Ok(())
}

impl Wallet {
    /// Prepare a NUT-18 payment request and freeze its selected mint fees.
    ///
    /// The returned payment must be explicitly completed with
    /// [`PreparedPaymentRequest::confirm`] or released with
    /// [`PreparedPaymentRequest::cancel`].
    #[instrument(skip_all)]
    pub async fn prepare_pay_request(
        &self,
        payment_request: PaymentRequest,
        custom_amount: Option<Amount>,
    ) -> Result<PreparedPaymentRequest, Error> {
        let unit = payment_request_unit(&payment_request)?;
        let requested_amount = match payment_request.amount {
            Some(amount) => amount,
            None => match custom_amount {
                Some(a) => a,
                None => return Err(Error::AmountUndefined),
            },
        };

        if unit != self.unit {
            return Err(Error::UnsupportedUnit);
        }

        if payment_request_mint_list_is_strict(&payment_request)
            && payment_request_uses_unlisted_mint(&payment_request, &self.mint_url)
        {
            return Err(Error::Custom(format!(
                "Mint {} is not accepted by this payment request. Accepted mints: {:?}",
                self.mint_url, payment_request.mints
            )));
        }

        let selected_method = wallet_payment_request_method(self, &payment_request, &unit)
            .await?
            .ok_or(Error::UnsupportedPaymentMethod)?;
        let method_fee = if payment_request_method_fee_applies(&payment_request, &self.mint_url) {
            selected_method.fee
        } else {
            Amount::ZERO
        };
        let payment_amount = requested_amount
            .checked_add(method_fee)
            .ok_or(Error::AmountOverflow)?;

        // Extract optional NUT-10 spending conditions from the payment request.
        //
        // NUT-18 encodes spending conditions in the optional `nut10` field using
        // `Nut10SecretRequest` (kind + data + tags). To actually create locked
        // ecash, we need full NUT-10 secrets, so we:
        //   1. Convert `Nut10SecretRequest` -> `Nut10Secret` (adds nonce, keeps tags)
        //   2. Convert `Nut10Secret` -> `SpendingConditions` (NUT-11 helper)
        let conditions = if let Some(nut10_request) = &payment_request.nut10 {
            let secret: Nut10Secret = nut10_request.clone().into();
            Some(SpendingConditions::try_from(secret)?)
        } else {
            None
        };

        // Prefer Nostr to avoid revealing the payer's IP address, then fall back
        // to HTTP POST when Nostr is unavailable in this build or request.
        let transport = payment_request_transport(&payment_request.transports)
            .cloned()
            .ok_or_else(|| {
                Error::Custom("No transport available in payment request".to_string())
            })?;

        let prepared_send = self
            .prepare_send(
                payment_amount,
                SendOptions {
                    conditions,
                    include_fee: true,
                    ..Default::default()
                },
            )
            .await?;

        let input_fee = match prepared_send
            .swap_fee()
            .checked_add(prepared_send.send_fee())
        {
            Some(input_fee) => input_fee,
            None => {
                prepared_send.cancel().await?;
                return Err(Error::AmountOverflow);
            }
        };
        let total_amount = match payment_amount.checked_add(input_fee) {
            Some(total_amount) => total_amount,
            None => {
                prepared_send.cancel().await?;
                return Err(Error::AmountOverflow);
            }
        };

        Ok(PreparedPaymentRequest {
            wallet: self.clone(),
            payment_request,
            transport,
            operation_id: prepared_send.operation_id(),
            requested_amount,
            method: selected_method.method,
            method_fee,
            payment_amount,
            swap_fee: prepared_send.swap_fee(),
            send_fee: prepared_send.send_fee(),
            input_fee,
            total_amount,
            send_options: prepared_send.options().clone(),
            proofs_to_swap: prepared_send.proofs_to_swap().clone(),
            proofs_to_send: prepared_send.proofs_to_send().clone(),
        })
    }

    #[instrument(skip_all)]
    async fn deliver_payment_request(
        &self,
        payment_request: &PaymentRequest,
        transport: &Transport,
        token: &crate::nuts::Token,
    ) -> Result<(), Error> {
        // We need the keysets information to properly convert from token proof to proof
        let proofs = self.token_proofs(token).await?;

        let payload = PaymentRequestPayload {
            id: payment_request.payment_id.clone(),
            memo: None,
            mint: self.mint_url.clone(),
            unit: self.unit.clone(),
            proofs,
        };

        match transport._type {
            TransportType::Nostr => {
                #[cfg(feature = "nostr")]
                {
                    let keys = Keys::generate();
                    let client = NostrClient::new(keys.clone());
                    let nprofile = Nip19Profile::from_bech32(&transport.target)
                        .map_err(|e| Error::Custom(format!("Invalid nprofile: {e}")))?;

                    let rumor = EventBuilder::new(
                        nostr_sdk::Kind::from_u16(14),
                        serde_json::to_string(&payload)
                            .map_err(|e| Error::Custom(format!("Serialize payload: {e}")))?,
                    )
                    .build(keys.public_key);
                    let relays = nprofile.relays;

                    for relay in relays.iter() {
                        client
                            .add_write_relay(relay)
                            .await
                            .map_err(|e| Error::Custom(format!("Add relay {relay}: {e}")))?;
                    }

                    client.connect().await;

                    let gift_wrap = client
                        .gift_wrap_to(relays, &nprofile.public_key, rumor, None)
                        .await
                        .map_err(|e| Error::Custom(format!("Publish Nostr event: {e}")))?;

                    if !gift_wrap.failed.is_empty() {
                        tracing::warn!(
                            "Could not publish to {}",
                            gift_wrap
                                .failed
                                .keys()
                                .map(|relay| relay.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }

                    ensure_nostr_delivery_succeeded(&gift_wrap)?;

                    tracing::info!(
                        "Published event {} successfully to {}",
                        gift_wrap.val,
                        gift_wrap
                            .success
                            .iter()
                            .map(|s| s.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );

                    Ok(())
                }
                #[cfg(not(feature = "nostr"))]
                Err(Error::Custom(
                    "Nostr is not enabled in this build".to_string(),
                ))
            }

            TransportType::HttpPost => {
                let client = HttpClient::new();

                let res = client
                    .post(&transport.target)
                    .json(&payload)
                    .send()
                    .await
                    .map_err(|e| Error::HttpError(None, e.to_string()))?;

                if res.is_success() {
                    tracing::info!("Successfully posted payment");
                    Ok(())
                } else {
                    let status = res.status();
                    let body = res.text().await.unwrap_or_default();
                    Err(Error::HttpError(Some(status), body))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    async fn test_repository() -> WalletRepository {
        use cdk_common::database::{Error as DatabaseError, WalletDatabase};

        let localstore: Arc<dyn WalletDatabase<DatabaseError> + Send + Sync> = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("in-memory database"),
        );

        crate::wallet::WalletRepositoryBuilder::new()
            .localstore(localstore)
            .seed([0u8; 64])
            .build()
            .await
            .expect("repository")
    }

    /// A request advertising the same key twice would build a lock the payer
    /// cannot satisfy, so it has to fail on the requester's side. Mirrors what
    /// `create_request` does with the parsed conditions.
    #[tokio::test]
    async fn get_pr_spending_conditions_rejects_duplicate_pubkey() {
        let pubkey = crate::nuts::SecretKey::generate().public_key().to_hex();
        let params = CreateRequestParams {
            pubkeys: Some(vec![pubkey.clone(), pubkey]),
            ..Default::default()
        };

        let conditions = test_repository()
            .await
            .get_pr_spending_conditions(&params)
            .expect("params should parse")
            .expect("conditions should be present");

        let err = Error::from(
            Nut10SecretRequest::try_from(conditions)
                .expect_err("duplicate pubkey should be rejected"),
        );

        assert!(
            matches!(
                err,
                Error::NUT11(crate::nuts::nut11::Error::DuplicatePubkey)
            ),
            "Expected DuplicatePubkey, got: {err:?}"
        );
    }

    #[test]
    fn create_request_params_default_is_strict_by_default() {
        let params = CreateRequestParams::default();

        assert_eq!(params.unit, "sat");
        assert_eq!(params.num_sigs, 1);
        assert_eq!(params.transport, "none");
        assert!(params.amount.is_none());
        assert!(params.mint_preferred.is_none());
        assert!(params.supported_methods.is_empty());
    }

    #[test]
    fn create_request_rejects_invalid_mint_urls() {
        let result = parse_payment_request_mints(Some(&["not a URL".to_string()]));

        assert!(result.is_err());
    }

    #[cfg(feature = "nostr")]
    #[tokio::test]
    async fn create_request_exposes_supported_method_fees() {
        let params = CreateRequestParams {
            amount: Some(100),
            supported_methods: vec![SupportedMethod::with_fee("bolt11", 5)],
            ..Default::default()
        };

        let (request, wait_info) = test_repository()
            .await
            .create_request(params)
            .await
            .expect("create request");

        assert_eq!(
            request.supported_methods,
            vec![SupportedMethod::with_fee("bolt11", 5)]
        );
        assert!(wait_info.is_none());
    }

    #[cfg(not(feature = "nostr"))]
    #[tokio::test]
    async fn create_request_exposes_supported_method_fees() {
        let params = CreateRequestParams {
            amount: Some(100),
            supported_methods: vec![SupportedMethod::with_fee("bolt11", 5)],
            ..Default::default()
        };

        let request = test_repository()
            .await
            .create_request(params)
            .await
            .expect("create request");

        assert_eq!(
            request.supported_methods,
            vec![SupportedMethod::with_fee("bolt11", 5)]
        );
    }

    #[test]
    fn payment_request_rejects_missing_unit_with_amount() {
        let payment_request = payment_request(None, Some(Amount::from(1)), vec![]);

        assert!(matches!(
            payment_request_unit(&payment_request),
            Err(Error::InvalidPaymentRequest)
        ));
    }

    #[test]
    fn payment_request_rejects_missing_unit_with_supported_methods() {
        let payment_request = payment_request(
            None,
            None,
            vec![SupportedMethod::new(PaymentMethod::BOLT11.to_string())],
        );

        assert!(matches!(
            payment_request_unit(&payment_request),
            Err(Error::InvalidPaymentRequest)
        ));
    }

    #[test]
    fn legacy_amountless_request_without_unit_defaults_to_sats() {
        let payment_request = payment_request(None, None, vec![]);

        assert_eq!(
            payment_request_unit(&payment_request).expect("legacy unit"),
            CurrencyUnit::Sat
        );
    }

    #[test]
    fn strict_mint_policy_only_accepts_listed_mints() {
        let listed_mint = MintUrl::from_str("https://listed.example.com").expect("valid URL");
        let unlisted_mint = MintUrl::from_str("https://unlisted.example.com").expect("valid URL");
        let mints = vec![listed_mint.clone()];

        assert!(payment_request_mint_policy_accepts_mint(
            &mints,
            None,
            &listed_mint
        ));
        assert!(!payment_request_mint_policy_accepts_mint(
            &mints,
            None,
            &unlisted_mint
        ));
        assert!(!payment_request_mint_policy_accepts_mint(
            &mints,
            Some(false),
            &unlisted_mint
        ));
    }

    #[test]
    fn preferred_or_empty_mint_policy_accepts_unlisted_mints() {
        let listed_mint = MintUrl::from_str("https://listed.example.com").expect("valid URL");
        let unlisted_mint = MintUrl::from_str("https://unlisted.example.com").expect("valid URL");
        let mints = vec![listed_mint];

        assert!(payment_request_mint_policy_accepts_mint(
            &mints,
            Some(true),
            &unlisted_mint
        ));
        assert!(payment_request_mint_policy_accepts_mint(
            &[],
            None,
            &unlisted_mint
        ));
    }

    #[test]
    fn method_fee_defaults_to_zero_when_request_has_no_method_restriction() {
        let fee = payment_request_method_fee_from_melt_methods(&[], &[], false, &CurrencyUnit::Sat)
            .expect("fee")
            .map(|selection| selection.fee);

        assert_eq!(fee, Some(Amount::ZERO));
    }

    #[test]
    fn method_fee_uses_lowest_supported_melt_method_fee() {
        let supported_methods = vec![
            SupportedMethod {
                method: PaymentMethod::BOLT11.to_string(),
                fee: Some(Amount::from(4)),
            },
            SupportedMethod {
                method: PaymentMethod::BOLT12.to_string(),
                fee: Some(Amount::from(2)),
            },
        ];
        let melt_methods = vec![
            melt_method(PaymentMethod::BOLT11, CurrencyUnit::Sat),
            melt_method(PaymentMethod::BOLT12, CurrencyUnit::Sat),
        ];

        let fee = payment_request_method_fee_from_melt_methods(
            &supported_methods,
            &melt_methods,
            false,
            &CurrencyUnit::Sat,
        )
        .expect("fee")
        .map(|selection| selection.fee);

        assert_eq!(fee, Some(Amount::from(2)));
    }

    #[test]
    fn method_fee_ignores_melt_methods_for_other_units() {
        let supported_methods = vec![SupportedMethod {
            method: PaymentMethod::BOLT11.to_string(),
            fee: Some(Amount::from(4)),
        }];
        let melt_methods = vec![melt_method(PaymentMethod::BOLT11, CurrencyUnit::Usd)];

        let fee = payment_request_method_fee_from_melt_methods(
            &supported_methods,
            &melt_methods,
            false,
            &CurrencyUnit::Sat,
        )
        .expect("fee")
        .map(|selection| selection.fee);

        assert_eq!(fee, None);
    }

    #[test]
    fn method_fee_requires_matching_melt_method() {
        let supported_methods = vec![SupportedMethod {
            method: PaymentMethod::BOLT11.to_string(),
            fee: Some(Amount::from(4)),
        }];
        let melt_methods = vec![melt_method(PaymentMethod::BOLT12, CurrencyUnit::Sat)];

        let fee = payment_request_method_fee_from_melt_methods(
            &supported_methods,
            &melt_methods,
            false,
            &CurrencyUnit::Sat,
        )
        .expect("fee")
        .map(|selection| selection.fee);

        assert_eq!(fee, None);
    }

    #[test]
    fn method_fee_rejects_methods_when_melting_is_disabled() {
        let supported_methods = vec![SupportedMethod {
            method: PaymentMethod::BOLT11.to_string(),
            fee: Some(Amount::from(4)),
        }];
        let melt_methods = vec![melt_method(PaymentMethod::BOLT11, CurrencyUnit::Sat)];

        let fee = payment_request_method_fee_from_melt_methods(
            &supported_methods,
            &melt_methods,
            true,
            &CurrencyUnit::Sat,
        )
        .expect("fee")
        .map(|selection| selection.fee);

        assert_eq!(fee, None);
    }

    #[test]
    fn method_fee_selection_includes_the_selected_method() {
        let supported_methods = vec![
            SupportedMethod::with_fee(PaymentMethod::BOLT11.to_string(), Amount::from(5)),
            SupportedMethod::with_fee(PaymentMethod::BOLT12.to_string(), Amount::from(2)),
        ];
        let melt_methods = vec![
            melt_method(PaymentMethod::BOLT11, CurrencyUnit::Sat),
            melt_method(PaymentMethod::BOLT12, CurrencyUnit::Sat),
        ];

        let selected = payment_request_method_fee_from_melt_methods(
            &supported_methods,
            &melt_methods,
            false,
            &CurrencyUnit::Sat,
        )
        .expect("method selection")
        .expect("supported method");

        assert_eq!(selected.method.as_deref(), Some("bolt12"));
        assert_eq!(selected.fee, Amount::from(2));
    }

    #[test]
    fn default_limits_allow_reviewed_fees() {
        let result = check_payment_request_limits(
            Amount::from(1),
            Amount::from(101),
            PayRequestOptions::default(),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn explicit_limits_accept_fees_at_the_boundary() {
        let result = check_payment_request_limits(
            Amount::from(500),
            Amount::from(602),
            PayRequestOptions {
                max_method_fee: Some(Amount::from(500)),
                max_total_amount: Some(Amount::from(602)),
            },
        );

        assert!(result.is_ok());
    }

    #[test]
    fn total_amount_limit_includes_method_and_input_fees() {
        let result = check_payment_request_limits(
            Amount::from(500),
            Amount::from(602),
            PayRequestOptions {
                max_method_fee: Some(Amount::from(500)),
                max_total_amount: Some(Amount::from(601)),
            },
        );

        assert!(matches!(result, Err(Error::MaxFeeExceeded)));
    }

    #[test]
    fn delivery_failure_error_preserves_the_pending_send_operation_id() {
        let operation_id = uuid::Uuid::new_v4();
        let error = payment_request_delivery_result(
            operation_id,
            Err(Error::Custom("transport failed".to_string())),
        )
        .expect_err("delivery failure");

        match error {
            Error::PaymentRequestDeliveryFailed {
                operation_id: failed_operation_id,
                source,
            } => {
                assert_eq!(failed_operation_id, operation_id);
                assert_eq!(source.to_string(), "`transport failed`");
            }
            error => panic!("unexpected error: {error}"),
        }
    }

    #[cfg(feature = "nostr")]
    #[test]
    fn nostr_delivery_fails_when_all_relays_fail() {
        let relay = RelayUrl::parse("wss://relay.example.com").expect("valid relay URL");
        let gift_wrap = Output {
            val: EventId::all_zeros(),
            success: Default::default(),
            failed: [(relay, "relay rejected event".to_string())]
                .into_iter()
                .collect(),
        };

        let error = ensure_nostr_delivery_succeeded(&gift_wrap).expect_err("delivery failure");

        match error {
            Error::NostrPublishFailed {
                event_id,
                failed_relays,
            } => {
                assert_eq!(event_id, EventId::all_zeros().to_string());
                assert_eq!(failed_relays, vec!["wss://relay.example.com".to_string()]);
            }
            error => panic!("unexpected error: {error}"),
        }
    }

    #[test]
    fn method_fee_only_applies_to_nonpreferred_mints() {
        let listed = MintUrl::from_str("https://listed.example.com").expect("valid URL");
        let unlisted = MintUrl::from_str("https://unlisted.example.com").expect("valid URL");
        let mut request = payment_request(Some(CurrencyUnit::Sat), Some(Amount::from(1)), vec![]);

        assert!(payment_request_method_fee_applies(&request, &listed));

        request.mints.push(listed.clone());
        assert!(!payment_request_method_fee_applies(&request, &listed));
        assert!(payment_request_method_fee_applies(&request, &unlisted));
    }

    #[test]
    fn payment_request_prefers_nostr_transport_for_privacy() {
        let http = Transport {
            _type: TransportType::HttpPost,
            target: "https://receiver.example.com".to_string(),
            tags: vec![],
        };
        let nostr = Transport {
            _type: TransportType::Nostr,
            target: "nprofile1example".to_string(),
            tags: vec![vec!["n".to_string(), "17".to_string()]],
        };

        #[cfg(feature = "nostr")]
        {
            assert_eq!(
                payment_request_transport(&[http.clone(), nostr.clone()]),
                Some(&nostr)
            );
            assert_eq!(
                payment_request_transport(&[nostr.clone(), http.clone()]),
                Some(&nostr)
            );
        }
        #[cfg(not(feature = "nostr"))]
        {
            assert_eq!(
                payment_request_transport(&[http.clone(), nostr.clone()]),
                Some(&http)
            );
            assert_eq!(
                payment_request_transport(&[nostr, http.clone()]),
                Some(&http)
            );
        }
    }

    #[tokio::test]
    async fn prepared_payment_exposes_exact_fee_breakdown_and_cancel_releases_proofs() {
        use crate::wallet::test_utils::{
            create_test_db, create_test_wallet_with_mock, test_keyset_id, test_mint_url,
            test_proof_info, MockMintConnector,
        };

        let db = create_test_db().await;
        db.update_proofs(
            vec![test_proof_info(test_keyset_id(), 1024, test_mint_url())],
            vec![],
        )
        .await
        .expect("store proof");
        let wallet = create_test_wallet_with_mock(db, Arc::new(MockMintConnector::new())).await;
        let request = payment_request_with_http_transport(Amount::from(100), Amount::from(500));

        let prepared = wallet
            .prepare_pay_request(request, None)
            .await
            .expect("prepare payment request");

        assert_eq!(prepared.requested_amount(), Amount::from(100));
        assert_eq!(prepared.method(), Some("bolt11"));
        assert_eq!(prepared.method_fee(), Amount::from(500));
        assert_eq!(prepared.payment_amount(), Amount::from(600));
        assert_eq!(
            prepared.total_amount(),
            prepared.payment_amount() + prepared.input_fee()
        );
        assert!(prepared.input_fee() > Amount::ZERO);
        assert!(wallet.total_reserved_balance().await.expect("balance") > Amount::ZERO);

        prepared.cancel().await.expect("cancel prepared payment");

        assert_eq!(
            wallet.total_reserved_balance().await.expect("balance"),
            Amount::ZERO
        );
        assert_eq!(
            wallet.total_balance().await.expect("balance"),
            Amount::from(1024)
        );
    }

    fn payment_request(
        unit: Option<CurrencyUnit>,
        amount: Option<Amount>,
        supported_methods: Vec<SupportedMethod>,
    ) -> PaymentRequest {
        PaymentRequest {
            payment_id: None,
            amount,
            unit,
            single_use: None,
            mints: vec![],
            mint_preferred: None,
            supported_methods,
            description: None,
            transports: vec![],
            nut10: None,
        }
    }

    fn payment_request_with_http_transport(amount: Amount, method_fee: Amount) -> PaymentRequest {
        PaymentRequest {
            payment_id: None,
            amount: Some(amount),
            unit: Some(CurrencyUnit::Sat),
            single_use: None,
            mints: vec![],
            mint_preferred: None,
            supported_methods: vec![SupportedMethod::with_fee(
                PaymentMethod::BOLT11.to_string(),
                method_fee,
            )],
            description: None,
            transports: vec![Transport {
                _type: TransportType::HttpPost,
                target: "https://receiver.example.com".to_string(),
                tags: vec![],
            }],
            nut10: None,
        }
    }

    fn melt_method(method: PaymentMethod, unit: CurrencyUnit) -> MeltMethodSettings {
        MeltMethodSettings {
            method,
            unit,
            method_name: None,
            min_amount: None,
            max_amount: None,
            options: None,
        }
    }
}

fn payment_request_unit(payment_request: &PaymentRequest) -> Result<CurrencyUnit, Error> {
    match &payment_request.unit {
        Some(unit) => Ok(unit.clone()),
        None if payment_request.amount.is_none()
            && payment_request.supported_methods.is_empty() =>
        {
            Ok(CurrencyUnit::Sat)
        }
        None => Err(Error::InvalidPaymentRequest),
    }
}

fn payment_request_mint_list_is_strict(payment_request: &PaymentRequest) -> bool {
    payment_request_mint_policy_is_strict(&payment_request.mints, payment_request.mint_preferred)
}

fn payment_request_mint_policy_is_strict(mints: &[MintUrl], mint_preferred: Option<bool>) -> bool {
    !mints.is_empty() && mint_preferred != Some(true)
}

#[cfg(any(feature = "nostr", test))]
fn payment_request_mint_policy_accepts_mint(
    mints: &[MintUrl],
    mint_preferred: Option<bool>,
    mint_url: &MintUrl,
) -> bool {
    !payment_request_mint_policy_is_strict(mints, mint_preferred) || mints.contains(mint_url)
}

fn payment_request_uses_unlisted_mint(
    payment_request: &PaymentRequest,
    mint_url: &MintUrl,
) -> bool {
    !payment_request.mints.is_empty() && !payment_request.mints.contains(mint_url)
}

async fn payment_request_amount_for_wallet(
    amount: Amount,
    payment_request: &PaymentRequest,
    wallet: &Wallet,
    unit: &CurrencyUnit,
) -> Result<Amount, Error> {
    if payment_request_mint_list_is_strict(payment_request)
        && payment_request_uses_unlisted_mint(payment_request, &wallet.mint_url)
    {
        return Err(Error::Custom(format!(
            "Mint {} is not accepted by this payment request. Accepted mints: {:?}",
            wallet.mint_url, payment_request.mints
        )));
    }

    let method_fee = wallet_payment_request_method(wallet, payment_request, unit)
        .await?
        .ok_or(Error::UnsupportedPaymentMethod)?
        .fee;

    if payment_request_method_fee_applies(payment_request, &wallet.mint_url) {
        return amount.checked_add(method_fee).ok_or(Error::AmountOverflow);
    }

    Ok(amount)
}

fn payment_request_method_fee_applies(
    payment_request: &PaymentRequest,
    mint_url: &MintUrl,
) -> bool {
    payment_request.mints.is_empty() || !payment_request.mints.contains(mint_url)
}

fn payment_request_transport(transports: &[Transport]) -> Option<&Transport> {
    #[cfg(feature = "nostr")]
    if let Some(nostr) = transports
        .iter()
        .find(|transport| transport._type == TransportType::Nostr)
    {
        return Some(nostr);
    }

    transports
        .iter()
        .find(|transport| transport._type == TransportType::HttpPost)
}

fn parse_payment_request_mints(mints: Option<&[String]>) -> Result<Vec<MintUrl>, Error> {
    mints
        .unwrap_or_default()
        .iter()
        .map(|url| {
            MintUrl::from_str(url)
                .map_err(|err| Error::Custom(format!("Invalid mint URL `{url}`: {err}")))
        })
        .collect()
}

async fn wallet_payment_request_method(
    wallet: &Wallet,
    payment_request: &PaymentRequest,
    unit: &CurrencyUnit,
) -> Result<Option<SelectedPaymentMethod>, Error> {
    if payment_request.supported_methods.is_empty() {
        return Ok(Some(SelectedPaymentMethod {
            method: None,
            fee: Amount::ZERO,
        }));
    }

    let mint_info = wallet.load_mint_info().await?;

    payment_request_method_fee_from_melt_methods(
        &payment_request.supported_methods,
        &mint_info.nuts.nut05.methods,
        mint_info.nuts.nut05.disabled,
        unit,
    )
}

fn payment_request_method_fee_from_melt_methods(
    supported_methods: &[SupportedMethod],
    melt_methods: &[MeltMethodSettings],
    melting_disabled: bool,
    unit: &CurrencyUnit,
) -> Result<Option<SelectedPaymentMethod>, Error> {
    if supported_methods.is_empty() {
        return Ok(Some(SelectedPaymentMethod {
            method: None,
            fee: Amount::ZERO,
        }));
    }

    if melting_disabled {
        return Ok(None);
    }

    let requested_methods = supported_methods
        .iter()
        .map(|method| {
            PaymentMethod::from_str(&method.method).map(|payment_method| {
                (
                    payment_method,
                    method.method.clone(),
                    method.fee.unwrap_or(Amount::ZERO),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut selected_method: Option<SelectedPaymentMethod> = None;
    for (method, method_name, fee) in requested_methods {
        let melt_supports_method = melt_methods
            .iter()
            .any(|settings| settings.unit == *unit && settings.method == method);

        if melt_supports_method {
            selected_method = match selected_method {
                Some(current) if current.fee <= fee => Some(current),
                _ => Some(SelectedPaymentMethod {
                    method: Some(method_name),
                    fee,
                }),
            };
        }
    }

    Ok(selected_method)
}

/// Parameters for creating a PaymentRequest
///
/// This mirrors the CLI inputs and is used by `create_request` to build a
/// NUT-18 PaymentRequest. When `transport` is set to `nostr`, the function
/// also returns a `NostrWaitInfo` that can be passed to `wait_for_nostr_payment`.
#[derive(Debug, Clone)]
pub struct CreateRequestParams {
    /// Optional amount to request (in the smallest unit for the chosen currency unit)
    pub amount: Option<u64>,
    /// Currency unit string (e.g., "sat")
    pub unit: String,
    /// Optional human-readable description for the request
    pub description: Option<String>,
    /// Optional set of public keys for P2PK spending conditions (multisig supported)
    pub pubkeys: Option<Vec<String>>, // multiple P2PK pubkeys
    /// Required number of signatures if `pubkeys` is provided (defaults typically to 1)
    pub num_sigs: u64, // required signatures for P2PK
    /// Optional HTLC hash condition (mutually exclusive with `preimage`)
    pub hash: Option<String>, // HTLC hash
    /// Optional HTLC preimage (mutually exclusive with `hash`)
    pub preimage: Option<String>, // HTLC preimage
    /// Transport type for the request: "nostr", "http", or "none"
    pub transport: String, // "nostr", "http", or "none"
    /// Target URL for HTTP transport (required if `transport == http`)
    pub http_url: Option<String>, // when transport == http
    /// List of Nostr relay URLs to include in the nprofile (used if `transport == nostr`)
    pub nostr_relays: Option<Vec<String>>, // when transport == nostr
    /// Optional list of mint URLs the receiver accepts or prefers; `None` emits no mint list
    pub mints: Option<Vec<String>>,
    /// Whether the mint list is preferred rather than required
    pub mint_preferred: Option<bool>,
    /// Payment methods the payer's mint must support, with optional per-method fees
    pub supported_methods: Vec<SupportedMethod>,
}

impl Default for CreateRequestParams {
    fn default() -> Self {
        Self {
            amount: None,
            unit: "sat".to_string(),
            description: None,
            pubkeys: None,
            num_sigs: 1,
            hash: None,
            preimage: None,
            transport: "none".to_string(),
            http_url: None,
            nostr_relays: None,
            mints: None,
            mint_preferred: None,
            supported_methods: vec![],
        }
    }
}

/// Extra information needed to wait for an incoming Nostr payment
///
/// Returned by `create_request` when the transport is `nostr`. Pass this to
/// `wait_for_nostr_payment` to connect, subscribe, and receive the incoming
/// payment on the specified relays.
#[cfg(feature = "nostr")]
#[derive(Debug, Clone)]
pub struct NostrWaitInfo {
    /// Ephemeral keys used to connect to relays and unwrap the gift-wrapped event
    pub keys: Keys,
    /// Nostr relays to read from while waiting for the payment
    pub relays: Vec<String>,
    /// The recipient public key to subscribe to for incoming events
    pub pubkey: nostr_sdk::PublicKey,
    /// Mint URLs accepted or preferred by the original payment request
    pub mints: Vec<MintUrl>,
    /// Whether the original request's mint list is preferred instead of strict
    pub mint_preferred: Option<bool>,
}

impl WalletRepository {
    /// Select a wallet and prepare a NUT-18 payment request for review.
    ///
    /// This method selects an appropriate mint:
    /// - If `mint_url` is provided, it verifies the payment request accepts that mint
    ///   and uses it to pay.
    /// - If `mint_url` is None, it automatically selects the mint that:
    ///   1. Is accepted by the payment request (matches one of the request's mints, or request accepts any mint)
    ///   2. Has the highest balance among matching mints
    ///
    /// # Arguments
    ///
    /// * `payment_request` - The NUT-18 payment request to pay
    /// * `mint_url` - Optional specific mint to use. If None, automatically selects the best matching mint.
    /// * `custom_amount` - Custom amount to pay (required if payment request has no amount)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The payment request has no amount and no custom amount is provided
    /// - The specified mint is not accepted by the payment request
    /// - No matching mint has sufficient balance
    /// - No transport is available in the payment request
    ///
    /// The returned payment must be explicitly completed with
    /// [`PreparedPaymentRequest::confirm`] or released with
    /// [`PreparedPaymentRequest::cancel`].
    #[instrument(skip_all)]
    pub async fn prepare_pay_request(
        &self,
        payment_request: PaymentRequest,
        mint_url: Option<MintUrl>,
        custom_amount: Option<Amount>,
    ) -> Result<PreparedPaymentRequest, Error> {
        let unit = payment_request_unit(&payment_request)?;
        let amount = match payment_request.amount {
            Some(amount) => amount,
            None => match custom_amount {
                Some(a) => a,
                None => return Err(Error::AmountUndefined),
            },
        };

        // Get the list of mints accepted by the payment request (empty means any mint is accepted)
        let accepted_mints = &payment_request.mints;
        let mint_list_is_preferred = payment_request.mint_preferred == Some(true);

        // Select the wallet to use for payment
        let selected_wallet = if let Some(specified_mint) = &mint_url {
            // User specified a mint - verify it's accepted by strict payment requests.
            if payment_request_mint_list_is_strict(&payment_request)
                && !accepted_mints.contains(specified_mint)
            {
                return Err(Error::Custom(format!(
                    "Mint {} is not accepted by this payment request. Accepted mints: {:?}",
                    specified_mint, accepted_mints
                )));
            }

            // Get the wallet for the specified mint and unit
            self.get_wallet(specified_mint, &unit).await?
        } else {
            // No mint specified - find the best matching mint with highest balance
            let balances = self.get_balances().await?;
            let mut best_preferred_wallet: Option<Arc<Wallet>> = None;
            let mut best_preferred_balance = Amount::ZERO;
            let mut best_fallback_wallet: Option<Arc<Wallet>> = None;
            let mut best_fallback_balance = Amount::ZERO;

            for (wallet_key, balance) in balances.iter() {
                // Only consider wallets with matching unit
                if wallet_key.unit != unit {
                    continue;
                }

                let mint_is_listed =
                    accepted_mints.is_empty() || accepted_mints.contains(&wallet_key.mint_url);

                if !mint_is_listed && !mint_list_is_preferred {
                    continue;
                }

                let wallet = match self.get_wallet(&wallet_key.mint_url, &unit).await {
                    Ok(wallet) => wallet,
                    Err(err) => {
                        tracing::warn!(
                            "Skipping mint {} while selecting a payment-request wallet: {}",
                            wallet_key.mint_url,
                            err
                        );
                        continue;
                    }
                };

                let required_amount = match payment_request_amount_for_wallet(
                    amount,
                    &payment_request,
                    &wallet,
                    &unit,
                )
                .await
                {
                    Ok(required_amount) => required_amount,
                    Err(err) => {
                        tracing::warn!(
                            "Skipping mint {} after its payment-method probe failed: {}",
                            wallet_key.mint_url,
                            err
                        );
                        continue;
                    }
                };

                // Check balance meets requirements and is best so far
                if *balance < required_amount {
                    continue;
                }

                if mint_is_listed {
                    if *balance > best_preferred_balance {
                        best_preferred_balance = *balance;
                        best_preferred_wallet = Some(Arc::new(wallet));
                    }
                } else if *balance > best_fallback_balance {
                    best_fallback_balance = *balance;
                    best_fallback_wallet = Some(Arc::new(wallet));
                }
            }

            best_preferred_wallet
                .or(best_fallback_wallet)
                .map(|w| (*w).clone())
                .ok_or(Error::InsufficientFunds)?
        };

        // Freeze the selected wallet, applicable method fee, and exact input fees.
        selected_wallet
            .prepare_pay_request(payment_request, custom_amount)
            .await
    }

    /// Derive enforceable NUT-10 spending conditions from high-level request params.
    ///
    /// Why:
    /// - Centralizes translation of CLI/SDK inputs (P2PK multisig and HTLC variants) into
    ///   a single, canonical `SpendingConditions` shape so requests are consistent.
    /// - Prevents ambiguous construction by capping `num_sigs` to the number of provided keys
    ///   and rejecting malformed hashes/inputs early. Repeated keys are an error, not a cap.
    /// - Encourages safe defaults by selecting `SigFlag::SigInputs` and composing conditions
    ///   that can be verified by recipients and mints.
    ///
    /// Behavior notes (rationale):
    /// - If no P2PK or HTLC data is given, returns `Ok(None)` so callers emit a plain request
    ///   without additional constraints.
    /// - With `pubkeys` only, constructs P2PK-style conditions where the first key is used as
    ///   the primary spend key and the remainder contribute to multisig according to `num_sigs`.
    /// - With `hash` or `preimage`, constructs an HTLC condition, optionally embedding P2PK
    ///   conditions to require signatures in addition to the hash lock.
    ///
    /// Errors:
    /// - Invalid SHA-256 `hash` strings or invalid HTLC/P2PK parameterizations surface as errors
    ///   from parsing and `SpendingConditions` constructors.
    /// - Conditions are validated here, so a request advertising a key twice within one
    ///   signing pathway fails at creation with `DuplicatePubkey` rather than on the
    ///   payer's side.
    fn get_pr_spending_conditions(
        &self,
        params: &CreateRequestParams,
    ) -> Result<Option<SpendingConditions>, Error> {
        // Spending conditions
        let spending_conditions: Option<SpendingConditions> =
            if let Some(pubkey_strings) = &params.pubkeys {
                // parse pubkeys
                let mut parsed_pubkeys = Vec::new();
                for p in pubkey_strings {
                    if let Ok(pk) = crate::nuts::nut01::PublicKey::from_str(p) {
                        parsed_pubkeys.push(pk);
                    }
                }

                if parsed_pubkeys.is_empty() {
                    None
                } else {
                    let num_sigs = params.num_sigs.min(parsed_pubkeys.len() as u64);

                    if let Some(hash_str) = &params.hash {
                        let conditions = Conditions {
                            locktime: None,
                            pubkeys: Some(parsed_pubkeys),
                            refund_keys: None,
                            num_sigs: Some(num_sigs),
                            sig_flag: SigFlag::SigInputs,
                            num_sigs_refund: None,
                        };

                        match Sha256Hash::from_str(hash_str) {
                            Ok(hash) => Some(SpendingConditions::HTLCConditions {
                                data: hash,
                                conditions: Some(conditions),
                            }),
                            Err(err) => {
                                return Err(Error::Custom(format!("Error parsing hash: {err}")))
                            }
                        }
                    } else if let Some(preimage) = &params.preimage {
                        let conditions = Conditions {
                            locktime: None,
                            pubkeys: Some(parsed_pubkeys),
                            refund_keys: None,
                            num_sigs: Some(num_sigs),
                            sig_flag: SigFlag::SigInputs,
                            num_sigs_refund: None,
                        };

                        Some(SpendingConditions::new_htlc(
                            preimage.to_string(),
                            Some(conditions),
                        )?)
                    } else {
                        Some(SpendingConditions::new_p2pk(
                            *parsed_pubkeys.first().expect("not empty"),
                            Some(Conditions {
                                locktime: None,
                                pubkeys: Some(parsed_pubkeys[1..].to_vec()),
                                refund_keys: None,
                                num_sigs: Some(num_sigs),
                                sig_flag: SigFlag::SigInputs,
                                num_sigs_refund: None,
                            }),
                        ))
                    }
                }
            } else if let Some(hash_str) = &params.hash {
                match Sha256Hash::from_str(hash_str) {
                    Ok(hash) => Some(SpendingConditions::HTLCConditions {
                        data: hash,
                        conditions: None,
                    }),
                    Err(err) => return Err(Error::Custom(format!("Error parsing hash: {err}"))),
                }
            } else if let Some(preimage) = &params.preimage {
                Some(SpendingConditions::new_htlc(preimage.to_string(), None)?)
            } else {
                None
            };

        Ok(spending_conditions)
    }

    /// Create a NUT-18 PaymentRequest from high-level parameters.
    ///
    /// Why:
    /// - Ensures the CLI and SDKs construct requests consistently using wallet context.
    /// - Advertises available mints for the chosen unit so payers can select compatible proofs.
    /// - Optionally embeds a transport; Nostr is preferred to reduce IP exposure for the payer.
    ///
    /// Behavior summary (focus on rationale rather than steps):
    /// - Uses `unit` to discover mints with balances as a hint to senders (helps route payments without leaking more data than necessary).
    /// - Translates P2PK/multisig and HTLC inputs (pubkeys/num_sigs/hash/preimage) into a NUT-10 secret request so the receiver can enforce spending constraints.
    /// - For `transport == "nostr"`, generates ephemeral keys and an nprofile pointing at the chosen relays; returns `NostrWaitInfo` so callers can wait for the incoming payment without coupling construction and reception logic.
    /// - For `transport == "http"`, attaches the provided endpoint; for `none` or unknown, omits transports to let the caller deliver out-of-band.
    ///
    /// Returns:
    /// - `(PaymentRequest, Some(NostrWaitInfo))` when `transport == "nostr"`.
    /// - `(PaymentRequest, None)` otherwise.
    ///
    /// Errors when:
    /// - `unit` cannot be parsed, relay URLs are invalid, or P2PK/HTLC parameters are malformed.
    ///
    /// Notes:
    /// - Sets `single_use = true` to discourage replays.
    /// - Ephemeral Nostr keys are intentional; keep `NostrWaitInfo` only as long as needed for reception.
    #[cfg(feature = "nostr")]
    pub async fn create_request(
        &self,
        params: CreateRequestParams,
    ) -> Result<(PaymentRequest, Option<NostrWaitInfo>), Error> {
        // Parse the explicitly configured mint policy. No list means any mint.
        let mints = parse_payment_request_mints(params.mints.as_deref())?;

        // Transports
        let transport_type = params.transport.to_lowercase();
        let (transports, nostr_info): (Vec<Transport>, Option<NostrWaitInfo>) =
            match transport_type.as_str() {
                "nostr" => {
                    let keys = Keys::generate();
                    let relays = if let Some(custom_relays) = &params.nostr_relays {
                        if !custom_relays.is_empty() {
                            custom_relays.clone()
                        } else {
                            return Err(Error::Custom("No relays provided".to_string()));
                        }
                    } else {
                        return Err(Error::Custom("No relays provided".to_string()));
                    };

                    // Parse relay URLs for nprofile
                    let relay_urls = relays
                        .iter()
                        .map(|r| RelayUrl::parse(r))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| Error::Custom(format!("Couldn't parse relays: {e}")))?;

                    let nprofile =
                        nostr_sdk::nips::nip19::Nip19Profile::new(keys.public_key, relay_urls);
                    let nostr_transport = Transport {
                        _type: TransportType::Nostr,
                        target: nprofile.to_bech32().map_err(|e| {
                            Error::Custom(format!("Couldn't convert nprofile to bech32: {e}"))
                        })?,
                        tags: vec![vec!["n".to_string(), "17".to_string()]],
                    };

                    (
                        vec![nostr_transport],
                        Some(NostrWaitInfo {
                            keys,
                            relays,
                            pubkey: nprofile.public_key,
                            mints: mints.clone(),
                            mint_preferred: params.mint_preferred,
                        }),
                    )
                }
                "http" => {
                    if let Some(url) = &params.http_url {
                        let http_transport = Transport {
                            _type: TransportType::HttpPost,
                            target: url.clone(),
                            tags: vec![],
                        };
                        (vec![http_transport], None)
                    } else {
                        return Err(Error::Custom(
                            "HTTP transport requires an HTTP URL".to_string(),
                        ));
                    }
                }
                "none" => (vec![], None),
                _ => {
                    return Err(Error::Custom(format!(
                        "Unsupported payment request transport `{}`",
                        params.transport
                    )))
                }
            };

        let nut10 = self
            .get_pr_spending_conditions(&params)?
            .map(Nut10SecretRequest::try_from)
            .transpose()?;

        let req = PaymentRequest {
            payment_id: None,
            amount: params.amount.map(Amount::from),
            unit: Some(CurrencyUnit::from_str(&params.unit)?),
            single_use: Some(true),
            mints,
            mint_preferred: params.mint_preferred,
            supported_methods: params.supported_methods,
            description: params.description,
            transports,
            nut10,
        };

        Ok((req, nostr_info))
    }

    /// Create a NUT-18 PaymentRequest from high-level parameters (Nostr disabled build).
    ///
    /// Why:
    /// - Keep request construction consistent even when Nostr is not compiled in.
    /// - Still advertise available mints for the unit so payers can route proofs correctly.
    /// - Allow callers to attach an HTTP transport when out-of-band delivery is acceptable.
    ///
    /// Behavior notes:
    /// - Rejects `transport == "nostr"` early so callers can surface a clear UX error.
    /// - Encodes P2PK/multisig and HTLC constraints into a NUT-10 secret request for enforceable spending conditions.
    ///
    /// Returns the constructed PaymentRequest and sets `single_use = true` to discourage replay.
    #[cfg(not(feature = "nostr"))]
    pub async fn create_request(
        &self,
        params: CreateRequestParams,
    ) -> Result<PaymentRequest, Error> {
        // Parse the explicitly configured mint policy. No list means any mint.
        let mints = parse_payment_request_mints(params.mints.as_deref())?;

        // Transports
        let transport_type = params.transport.to_lowercase();
        let transports: Vec<Transport> = match transport_type.as_str() {
            "nostr" => {
                return Err(Error::Custom(
                    "Nostr is not supported in this build".to_string(),
                ))
            }
            "http" => {
                if let Some(url) = &params.http_url {
                    let http_transport = Transport {
                        _type: TransportType::HttpPost,
                        target: url.clone(),
                        tags: vec![],
                    };
                    vec![http_transport]
                } else {
                    return Err(Error::Custom(
                        "HTTP transport requires an HTTP URL".to_string(),
                    ));
                }
            }
            "none" => vec![],
            _ => {
                return Err(Error::Custom(format!(
                    "Unsupported payment request transport `{}`",
                    params.transport
                )))
            }
        };

        let nut10 = self
            .get_pr_spending_conditions(&params)?
            .map(Nut10SecretRequest::try_from)
            .transpose()?;

        let req = PaymentRequest {
            payment_id: None,
            amount: params.amount.map(Amount::from),
            unit: Some(CurrencyUnit::from_str(&params.unit)?),
            single_use: Some(true),
            mints,
            mint_preferred: params.mint_preferred,
            supported_methods: params.supported_methods,
            description: params.description,
            transports,
            nut10,
        };

        Ok(req)
    }

    /// Wait for a Nostr payment for the previously constructed PaymentRequest and receive it into the wallet.
    #[cfg(all(feature = "nostr", not(target_arch = "wasm32")))]
    pub async fn wait_for_nostr_payment(&self, info: NostrWaitInfo) -> Result<Amount> {
        use futures::StreamExt;

        use crate::wallet::streams::nostr::NostrPaymentEventStream;

        let NostrWaitInfo {
            keys,
            relays,
            pubkey,
            mints,
            mint_preferred,
        } = info;

        let mut stream = NostrPaymentEventStream::new(keys, relays, pubkey);
        let cancel = stream.cancel_token();

        // Optional: you may expose cancel to caller, or use a timeout here.
        // tokio::spawn(async move { tokio::time::sleep(Duration::from_secs(120)).await; cancel.cancel(); });

        while let Some(item) = stream.next().await {
            match item {
                Ok(payload) => {
                    if !payment_request_mint_policy_accepts_mint(
                        &mints,
                        mint_preferred,
                        &payload.mint,
                    ) {
                        continue;
                    }

                    let token = crate::nuts::Token::new(
                        payload.mint.clone(),
                        payload.proofs,
                        payload.memo,
                        payload.unit.clone(),
                    );

                    // Get or create wallet for the token's mint
                    let unit = payload.unit.clone();
                    let wallet = match self.get_wallet(&payload.mint, &unit).await {
                        Ok(w) => w,
                        Err(_) => self.create_wallet(payload.mint.clone(), unit, None).await?,
                    };

                    // Receive using the individual wallet
                    let token_str = token.to_string();
                    let received = wallet
                        .receive(&token_str, ReceiveOptions::default())
                        .await?;

                    // Stop after first successful receipt
                    cancel.cancel();
                    return Ok(received);
                }
                Err(_) => {
                    // Keep listening on parse errors; if you prefer fail-fast, return the error
                    continue;
                }
            }
        }

        // If stream ended without receiving a payment, return zero.
        Ok(Amount::ZERO)
    }

    /// Wait for a Nostr payment for the previously constructed PaymentRequest and receive it into the wallet.
    ///
    /// wasm32 fallback: Streams are not available; we await the first matching notification and process it.
    #[cfg(all(feature = "nostr", target_arch = "wasm32"))]
    pub async fn wait_for_nostr_payment(&self, info: NostrWaitInfo) -> Result<Amount> {
        use nostr_sdk::prelude::*;

        let NostrWaitInfo {
            keys,
            relays,
            pubkey,
            mints,
            mint_preferred,
        } = info;

        let client = nostr_sdk::Client::new(keys);

        for r in &relays {
            client
                .add_read_relay(r.clone())
                .await
                .map_err(|e| crate::error::Error::Custom(format!("Add relay {r}: {e}")))?;
        }

        client.connect().await;

        // Subscribe to events addressed to `pubkey`
        let filter = Filter::new().pubkey(pubkey);
        client
            .subscribe(filter, None)
            .await
            .map_err(|e| crate::error::Error::Custom(format!("Subscribe: {e}")))?;

        // Await notifications until we successfully parse a payment payload and receive it
        let mut notifications = client.notifications();
        while let Ok(notification) = notifications.recv().await {
            if let RelayPoolNotification::Event { event, .. } = notification {
                match client.unwrap_gift_wrap(&event).await {
                    Ok(unwrapped) => {
                        let rumor = unwrapped.rumor;
                        match serde_json::from_str::<PaymentRequestPayload>(&rumor.content) {
                            Ok(payload) => {
                                if !payment_request_mint_policy_accepts_mint(
                                    &mints,
                                    mint_preferred,
                                    &payload.mint,
                                ) {
                                    continue;
                                }

                                let token = crate::nuts::Token::new(
                                    payload.mint.clone(),
                                    payload.proofs,
                                    payload.memo,
                                    payload.unit.clone(),
                                );

                                // Get or create wallet for the token's mint
                                let unit = payload.unit.clone();
                                let wallet = match self.get_wallet(&payload.mint, &unit).await {
                                    Ok(w) => w,
                                    Err(_) => {
                                        self.create_wallet(payload.mint.clone(), unit, None).await?
                                    }
                                };

                                // Receive using the individual wallet
                                let token_str = token.to_string();
                                let received = wallet
                                    .receive(&token_str, ReceiveOptions::default())
                                    .await?;

                                return Ok(received);
                            }
                            Err(_) => {
                                // Ignore malformed payloads and continue listening
                                continue;
                            }
                        }
                    }
                    Err(_) => {
                        // Ignore unwrap errors and continue listening
                        continue;
                    }
                }
            }
        }

        Ok(Amount::ZERO)
    }
}
