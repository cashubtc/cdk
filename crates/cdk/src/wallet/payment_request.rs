//! Utilities for paying NUT-18 Payment Requests.
//!
//! This module prepares and broadcasts payments for Cashu NUT-18 payment requests using either
//! Nostr or HTTP transports when available. If no transport is present in the request, an error
//! is returned so callers can handle alternative delivery mechanisms explicitly.

use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use bitcoin::hashes::sha256::Hash as Sha256Hash;
use cdk_common::{Amount, HttpClient, PaymentRequest, PaymentRequestPayload, TransportType};
use cdk_http_client::RequestBuilderExt;
#[cfg(feature = "nostr")]
use nostr_sdk::nips::nip19::Nip19Profile;
#[cfg(feature = "nostr")]
use nostr_sdk::prelude::*;
#[cfg(feature = "nostr")]
use nostr_sdk::{Client as NostrClient, EventBuilder, FromBech32, Keys, ToBech32};

use crate::error::Error;
use crate::mint_url::MintUrl;
use crate::nuts::nut11::{Conditions, SigFlag, SpendingConditions};
use crate::nuts::nut18::Nut10SecretRequest;
use crate::nuts::{CurrencyUnit, Nut10Secret, Transport};
#[cfg(feature = "nostr")]
use crate::wallet::ReceiveOptions;
use crate::wallet::{SendOptions, WalletRepository};
use crate::Wallet;

impl Wallet {
    /// Pay a NUT-18 PaymentRequest using a specific wallet.
    ///
    /// - If the request contains a Nostr or HttpPost transport, it will try those (preferring Nostr).
    /// - If no usable transport is present, this returns an error.
    /// - If the request has no amount, a `custom_amount` must be provided.
    pub async fn pay_request(
        &self,
        payment_request: PaymentRequest,
        custom_amount: Option<Amount>,
    ) -> Result<(), Error> {
        let amount = match payment_request.amount {
            Some(amount) => amount,
            None => match custom_amount {
                Some(a) => a,
                None => return Err(Error::AmountUndefined),
            },
        };

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

        let transports = payment_request.transports.clone();

        // Prefer Nostr to avoid revealing IP, fall back to HTTP POST.
        let transport = transports
            .iter()
            .find(|t| t._type == TransportType::Nostr)
            .or_else(|| {
                transports
                    .iter()
                    .find(|t| t._type == TransportType::HttpPost)
            });

        let prepared_send = self
            .prepare_send(
                amount,
                SendOptions {
                    conditions,
                    include_fee: true,
                    ..Default::default()
                },
            )
            .await?;

        let token = prepared_send.confirm(None).await?;

        // We need the keysets information to properly convert from token proof to proof
        let keysets_info = match self.localstore.get_mint_keysets(token.mint_url()?).await? {
            Some(keysets_info) => keysets_info,
            None => self.load_mint_keysets().await?,
        };
        let proofs = token.proofs(&keysets_info)?;

        if let Some(transport) = transport {
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
                        let client = NostrClient::new(keys);
                        let nprofile = Nip19Profile::from_bech32(&transport.target)
                            .map_err(|e| Error::Custom(format!("Invalid nprofile: {e}")))?;

                        let rumor = EventBuilder::new(
                            nostr_sdk::Kind::from_u16(14),
                            serde_json::to_string(&payload)
                                .map_err(|e| Error::Custom(format!("Serialize payload: {e}")))?,
                        )
                        .build(nprofile.public_key);
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

                        println!(
                            "Published event {} successfully to {}",
                            gift_wrap.val,
                            gift_wrap
                                .success
                                .iter()
                                .map(|s| s.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        );

                        if !gift_wrap.failed.is_empty() {
                            println!(
                                "Could not publish to {}",
                                gift_wrap
                                    .failed
                                    .keys()
                                    .map(|relay| relay.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                        }

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

                    let status = res.status();
                    if res.is_success() {
                        println!("Successfully posted payment");
                        Ok(())
                    } else {
                        let body = res.text().await.unwrap_or_default();
                        Err(Error::HttpError(Some(status), body))
                    }
                }
            }
        } else {
            // If no transport is available, return an error instead of printing the token
            Err(Error::Custom(
                "No transport available in payment request".to_string(),
            ))
        }
    }
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
}

impl WalletRepository {
    /// Pay a NUT-18 PaymentRequest using the WalletRepository.
    ///
    /// This method handles paying a payment request by selecting an appropriate mint:
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
    pub async fn pay_request(
        &self,
        payment_request: PaymentRequest,
        mint_url: Option<MintUrl>,
        custom_amount: Option<Amount>,
    ) -> Result<(), Error> {
        let amount = match payment_request.amount {
            Some(amount) => amount,
            None => match custom_amount {
                Some(a) => a,
                None => return Err(Error::AmountUndefined),
            },
        };

        // Get the list of mints accepted by the payment request (None means any mint is accepted)
        let accepted_mints = payment_request.mints.as_ref();

        // Get the unit from the payment request, defaulting to Sat
        let unit = payment_request.unit.clone().unwrap_or(CurrencyUnit::Sat);

        // Select the wallet to use for payment
        let selected_wallet = if let Some(specified_mint) = &mint_url {
            // User specified a mint - verify it's accepted by the payment request
            if let Some(accepted) = accepted_mints {
                if !accepted.contains(specified_mint) {
                    return Err(Error::Custom(format!(
                        "Mint {} is not accepted by this payment request. Accepted mints: {:?}",
                        specified_mint, accepted
                    )));
                }
            }

            // Get the wallet for the specified mint and unit
            self.get_wallet(specified_mint, &unit).await?
        } else {
            // No mint specified - find the best matching mint with highest balance
            let balances = self.get_balances().await?;
            let mut best_wallet: Option<Arc<Wallet>> = None;
            let mut best_balance = Amount::ZERO;

            for (wallet_key, balance) in balances.iter() {
                // Only consider wallets with matching unit
                if wallet_key.unit != unit {
                    continue;
                }

                // Check if this mint is accepted by the payment request
                let is_accepted = match accepted_mints {
                    Some(accepted) => accepted.contains(&wallet_key.mint_url),
                    None => true, // No mints specified means any mint is accepted
                };

                if !is_accepted {
                    continue;
                }

                // Check balance meets requirements and is best so far
                if *balance >= amount && *balance > best_balance {
                    if let Ok(wallet) = self.get_wallet(&wallet_key.mint_url, &unit).await {
                        best_balance = *balance;
                        best_wallet = Some(Arc::new(wallet));
                    }
                }
            }

            best_wallet
                .map(|w| (*w).clone())
                .ok_or(Error::InsufficientFunds)?
        };

        // Use the selected wallet to pay the request
        selected_wallet
            .pay_request(payment_request, custom_amount)
            .await
    }

    /// Derive enforceable NUT-10 spending conditions from high-level request params.
    ///
    /// Why:
    /// - Centralizes translation of CLI/SDK inputs (P2PK multisig and HTLC variants) into
    ///   a single, canonical `SpendingConditions` shape so requests are consistent.
    /// - Prevents ambiguous construction by capping `num_sigs` to the number of provided keys
    ///   and rejecting malformed hashes/inputs early.
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
        // Collect available mints for the selected unit
        // Filter by the requested unit and extract unique mint URLs
        let requested_unit = CurrencyUnit::from_str(&params.unit)?;
        let mints: Vec<MintUrl> = self
            .get_balances()
            .await?
            .keys()
            .filter(|key| key.unit == requested_unit)
            .map(|key| key.mint_url.clone())
            .collect();

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
                        tags: Some(vec![vec!["n".to_string(), "17".to_string()]]),
                    };

                    (
                        vec![nostr_transport],
                        Some(NostrWaitInfo {
                            keys,
                            relays,
                            pubkey: nprofile.public_key,
                        }),
                    )
                }
                "http" => {
                    if let Some(url) = &params.http_url {
                        let http_transport = Transport {
                            _type: TransportType::HttpPost,
                            target: url.clone(),
                            tags: None,
                        };
                        (vec![http_transport], None)
                    } else {
                        // No URL provided, skip transport
                        (vec![], None)
                    }
                }
                "none" => (vec![], None),
                _ => (vec![], None),
            };

        let nut10 = self
            .get_pr_spending_conditions(&params)?
            .map(Nut10SecretRequest::from);

        let req = PaymentRequest {
            payment_id: None,
            amount: params.amount.map(Amount::from),
            unit: Some(CurrencyUnit::from_str(&params.unit)?),
            single_use: Some(true),
            mints: Some(mints),
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
        // Collect available mints for the selected unit
        // Filter by the requested unit and extract unique mint URLs
        let requested_unit = CurrencyUnit::from_str(&params.unit)?;
        let mints: Vec<MintUrl> = self
            .get_balances()
            .await?
            .keys()
            .filter(|key| key.unit == requested_unit)
            .map(|key| key.mint_url.clone())
            .collect();

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
                        tags: None,
                    };
                    vec![http_transport]
                } else {
                    // No URL provided, skip transport
                    vec![]
                }
            }
            _ => vec![],
        };

        let nut10 = self
            .get_pr_spending_conditions(&params)?
            .map(Nut10SecretRequest::from);

        let req = PaymentRequest {
            payment_id: None,
            amount: params.amount.map(Amount::from),
            unit: Some(CurrencyUnit::from_str(&params.unit)?),
            single_use: Some(true),
            mints: Some(mints),
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
        } = info;

        let mut stream = NostrPaymentEventStream::new(keys, relays, pubkey);
        let cancel = stream.cancel_token();

        // Optional: you may expose cancel to caller, or use a timeout here.
        // tokio::spawn(async move { tokio::time::sleep(Duration::from_secs(120)).await; cancel.cancel(); });

        while let Some(item) = stream.next().await {
            match item {
                Ok(payload) => {
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
