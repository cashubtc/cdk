//! Utilities for paying NUT-18 Payment Requests.
//!
//! This module prepares and broadcasts payments for Cashu NUT-18 payment requests using either
//! Nostr or HTTP transports when available. If no transport is present in the request, an error
//! is returned so callers can handle alternative delivery mechanisms explicitly.

use cdk_common::{Amount, PaymentRequest, PaymentRequestPayload, TransportType};
use nostr_sdk::nips::nip19::Nip19Profile;
use nostr_sdk::{Client as NostrClient, EventBuilder, FromBech32, Keys};
use reqwest::Client;

use crate::wallet::SendOptions;
use crate::Wallet;

/// Pay a NUT-18 PaymentRequest using a specific wallet.
///
/// - If the request contains a Nostr or HttpPost transport, it will try those (preferring Nostr).
/// - If no usable transport is present, this returns an error.
/// - If the request has no amount, a `custom_amount` must be provided.
pub async fn pay_request(
    payment_request: PaymentRequest,
    matching_wallet: &Wallet,
    custom_amount: Option<Amount>,
) -> Result<(), crate::error::Error> {
    use crate::error::Error;

    let amount = match payment_request.amount {
        Some(amount) => amount,
        None => match custom_amount {
            Some(a) => a,
            None => return Err(Error::AmountUndefined),
        },
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

    let prepared_send = matching_wallet
        .prepare_send(
            amount,
            SendOptions {
                include_fee: true,
                ..Default::default()
            },
        )
        .await?;

    let token = prepared_send.confirm(None).await?;

    // We need the keysets information to properly convert from token proof to proof
    let keysets_info = match matching_wallet
        .localstore
        .get_mint_keysets(token.mint_url()?)
        .await?
    {
        Some(keysets_info) => keysets_info,
        None => matching_wallet.load_mint_keysets().await?, // Hit the keysets endpoint if we don't have the keysets for this Mint
    };
    let proofs = token.proofs(&keysets_info)?;

    if let Some(transport) = transport {
        let payload = PaymentRequestPayload {
            id: payment_request.payment_id.clone(),
            memo: None,
            mint: matching_wallet.mint_url.clone(),
            unit: matching_wallet.unit.clone(),
            proofs,
        };

        match transport._type {
            TransportType::Nostr => {
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

            TransportType::HttpPost => {
                let client = Client::new();

                let res = client
                    .post(transport.target.clone())
                    .json(&payload)
                    .send()
                    .await
                    .map_err(|e| Error::HttpError(None, e.to_string()))?;

                let status = res.status();
                if status.is_success() {
                    println!("Successfully posted payment");
                    Ok(())
                } else {
                    let body = res.text().await.unwrap_or_default();
                    Err(Error::HttpError(Some(status.as_u16()), body))
                }
            }
        }
    } else {
        // If no transport is available, return an error instead of printing the token
        return Err(Error::Custom(
            "No transport available in payment request".to_string(),
        ));
    }
}
