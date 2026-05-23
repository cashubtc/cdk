//! Onchain Regtest Integration Tests
//!
//! This file contains tests for NUT-26 onchain payments against a regtest environment.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use bip39::Mnemonic;
use cdk::amount::SplitTarget;
#[cfg(feature = "payjoin-regtest")]
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::{CurrencyUnit, NotificationPayload, PaymentMethod, Proofs, ProofsMethods};
#[cfg(feature = "payjoin-regtest")]
use cdk::nuts::{
    MeltQuoteOnchainRequest, MeltQuoteOnchainResponse, MintQuoteOnchainRequest,
    MintQuoteOnchainResponse, SecretKey,
};
use cdk::wallet::{MeltOutcome, MintConnector, Wallet, WalletSubscription};
#[cfg(feature = "payjoin-regtest")]
use cdk_common::payjoin::{format_bip21_amount_from_sats, payjoin_v2_to_bip77_endpoint};
use cdk_integration_tests::get_mint_url_from_env;
#[cfg(feature = "payjoin-regtest")]
use cdk_integration_tests::get_second_mint_url_from_env;
use cdk_integration_tests::init_regtest::init_bitcoin_client;
use cdk_sqlite::wallet::memory;
use futures::StreamExt;
use tokio::time::timeout;

#[cfg(feature = "payjoin-regtest")]
async fn request_payjoin_mint_quote(
    mint_url: &str,
) -> anyhow::Result<MintQuoteOnchainResponse<String>> {
    let request = MintQuoteOnchainRequest {
        unit: CurrencyUnit::Sat,
        pubkey: SecretKey::generate().public_key(),
    };
    let url = format!("{}/v1/mint/quote/onchain", mint_url.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .post(url)
        .json(&request)
        .send()
        .await?;
    let response = response.error_for_status()?;
    Ok(response.json().await?)
}

#[cfg(feature = "payjoin-regtest")]
async fn fetch_onchain_mint_quote(
    mint_url: &str,
    quote_id: &str,
) -> anyhow::Result<MintQuoteOnchainResponse<String>> {
    let url = format!(
        "{}/v1/mint/quote/onchain/{}",
        mint_url.trim_end_matches('/'),
        quote_id
    );
    let response = reqwest::Client::new().get(url).send().await?;
    let response = response.error_for_status()?;
    Ok(response.json().await?)
}

#[cfg(feature = "payjoin-regtest")]
async fn request_payjoin_melt_quote(
    mint_url: &str,
    destination_quote: &MintQuoteOnchainResponse<String>,
    amount_sat: u64,
) -> anyhow::Result<MeltQuoteOnchainResponse<String>> {
    let request = MeltQuoteOnchainRequest {
        request: destination_quote.request.clone(),
        unit: CurrencyUnit::Sat,
        amount: amount_sat.into(),
        payjoin: destination_quote.payjoin.clone(),
    };
    let url = format!("{}/v1/melt/quote/onchain", mint_url.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .post(url)
        .json(&request)
        .send()
        .await?;
    let response = response.error_for_status()?;
    Ok(response.json().await?)
}

#[cfg(feature = "payjoin-regtest")]
#[derive(Debug)]
struct NoopSenderPersister;

#[cfg(feature = "payjoin-regtest")]
impl payjoin::persist::SessionPersister for NoopSenderPersister {
    type InternalStorageError = std::io::Error;
    type SessionEvent = payjoin::send::v2::SessionEvent;

    fn save_event(&self, _event: Self::SessionEvent) -> Result<(), Self::InternalStorageError> {
        Ok(())
    }

    fn load(
        &self,
    ) -> Result<Box<dyn Iterator<Item = Self::SessionEvent>>, Self::InternalStorageError> {
        Ok(Box::new(std::iter::empty()))
    }

    fn close(&self) -> Result<(), Self::InternalStorageError> {
        Ok(())
    }
}

#[cfg(feature = "payjoin-regtest")]
async fn payjoin_http_request(request: payjoin::Request) -> anyhow::Result<Vec<u8>> {
    let response = reqwest::Client::new()
        .post(request.url)
        .header(reqwest::header::CONTENT_TYPE, request.content_type)
        .body(request.body)
        .send()
        .await?;
    let response = response.error_for_status()?;
    Ok(response.bytes().await?.to_vec())
}

#[cfg(feature = "payjoin-regtest")]
async fn send_payjoin_with_bitcoin_core(
    bitcoin_client: &cdk_integration_tests::ln_regtest::bitcoin_client::BitcoinClient,
    quote: &MintQuoteOnchainResponse<String>,
    amount_sat: u64,
) -> anyhow::Result<()> {
    use payjoin::bitcoin::FeeRate;
    use payjoin::persist::OptionalTransitionOutcome;
    use payjoin::UriExt;

    let payjoin = quote
        .payjoin
        .as_ref()
        .expect("Payjoin-enabled mint quote should include Payjoin params");
    let ohttp_relay_url = std::env::var("CDK_MINTD_BDK_PAYJOIN_OHTTP_RELAY_URL")
        .or_else(|_| std::env::var("CDK_REGTEST_PAYJOIN_OHTTP_RELAY_URL"))?;

    let bip21 = format!(
        "bitcoin:{}?amount={}&pj={}",
        quote.request,
        format_bip21_amount_from_sats(amount_sat),
        url::form_urlencoded::byte_serialize(payjoin_v2_to_bip77_endpoint(payjoin)?.as_bytes())
            .collect::<String>()
    );
    let pj_uri = payjoin::Uri::try_from(bip21.as_str())
        .map_err(|err| anyhow::anyhow!("{err}"))?
        .assume_checked()
        .check_pj_supported()
        .map_err(|_| anyhow::anyhow!("Payjoin URI did not contain supported pj params"))?;

    let original_psbt = bitcoin_client.create_funded_psbt(&quote.request, amount_sat, 1)?;
    let original_psbt = bitcoin_client.sign_psbt(&original_psbt)?;
    let original_psbt = payjoin::bitcoin::Psbt::from_str(&original_psbt)?;
    let fee_rate = FeeRate::from_sat_per_vb_u32(1);
    let persister = NoopSenderPersister;

    let sender = payjoin::send::v2::SenderBuilder::new(original_psbt, pj_uri)
        .build_recommended(fee_rate)
        .map_err(|err| anyhow::anyhow!("{err}"))?
        .save(&persister)?;
    let (post_request, post_context) = sender.create_v2_post_request(&ohttp_relay_url)?;
    let post_response = payjoin_http_request(post_request).await?;
    let mut sender = sender
        .process_response(&post_response, post_context)
        .save(&persister)?;

    let proposal_psbt = timeout(Duration::from_secs(180), async {
        loop {
            let (get_request, get_context) = sender.create_poll_request(&ohttp_relay_url)?;
            let get_response = payjoin_http_request(get_request).await?;
            match sender
                .process_response(&get_response, get_context)
                .save(&persister)?
            {
                OptionalTransitionOutcome::Progress(psbt) => return anyhow::Ok(psbt),
                OptionalTransitionOutcome::Stasis(next_sender) => {
                    sender = next_sender;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    })
    .await??;

    let signed_psbt = bitcoin_client.sign_psbt(&proposal_psbt.to_string())?;
    bitcoin_client.finalize_and_broadcast_psbt(&signed_psbt)?;

    Ok(())
}

#[cfg(feature = "payjoin-regtest")]
async fn fund_wallet_with_onchain(
    wallet: &Wallet,
    bitcoin_client: &cdk_integration_tests::ln_regtest::bitcoin_client::BitcoinClient,
    amount_sat: u64,
) -> anyhow::Result<()> {
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain")?,
            Some(amount_sat.into()),
            None,
            None,
        )
        .await?;

    bitcoin_client.send_to_address(&mint_quote.request, amount_sat)?;
    let mine_addr = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&mine_addr, 1)?;

    wallet
        .wait_and_mint_quote(
            mint_quote,
            SplitTarget::default(),
            None,
            Duration::from_secs(60),
        )
        .await?;

    Ok(())
}

#[cfg(feature = "payjoin-regtest")]
fn selected_onchain_melt_quote(
    wallet: &Wallet,
    response: &MeltQuoteOnchainResponse<String>,
) -> anyhow::Result<cdk::wallet::MeltQuote> {
    let fee_option = response
        .fee_options
        .iter()
        .find(|option| option.fee_index == 1)
        .or_else(|| response.fee_options.first())
        .copied()
        .ok_or_else(|| anyhow::anyhow!("Payjoin melt quote did not include fee options"))?;

    Ok(cdk::wallet::MeltQuote {
        id: response.quote.clone(),
        mint_url: Some(wallet.mint_url.clone()),
        unit: response.unit.clone(),
        amount: response.amount,
        request: response.request.clone(),
        fee_reserve: fee_option.fee_reserve,
        state: response.state,
        expiry: response.expiry,
        payment_proof: response.outpoint.clone(),
        estimated_blocks: Some(fee_option.estimated_blocks),
        fee_index: Some(fee_option.fee_index),
        payjoin: response.payjoin.clone(),
        payment_method: PaymentMethod::Known(KnownMethod::Onchain),
        used_by_operation: None,
        version: 0,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_mint() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 10_000;

    // 1. Request a mint quote for onchain payment
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .expect("Failed to get mint quote");

    assert!(mint_quote.request.starts_with("bcrt1"));
    println!("Mint address: {}", mint_quote.request);

    // 2. Subscribe to notifications for this quote
    let mut subscription = wallet
        .subscribe(WalletSubscription::MintQuoteOnchainState(vec![mint_quote
            .id
            .clone()]))
        .await
        .expect("failed to subscribe");

    // 3. Send bitcoin to the mint address
    bitcoin_client
        .send_to_address(&mint_quote.request, mint_amount)
        .expect("Failed to send bitcoin");

    // 4. Mine a block to confirm the transaction
    let mine_addr = bitcoin_client
        .get_new_address()
        .expect("Failed to get address");
    bitcoin_client
        .generate_blocks(&mine_addr, 1)
        .expect("Failed to mine block");

    // 5. Wait for paid notification
    // The mint checks for confirmations. Since we set num_confs=1 in settings, 1 block should be enough.
    let mut paid_amount = cdk::amount::Amount::from(0);
    timeout(Duration::from_secs(30), async {
        while let Some(msg) = subscription.recv().await {
            match msg.into_inner() {
                NotificationPayload::MintQuoteOnchainResponse(response) => {
                    assert_eq!(response.quote, mint_quote.id);
                    if response.amount_paid == mint_amount.into() {
                        paid_amount = response.amount_paid;
                        return;
                    }
                }
                _ => panic!("Unexpected notification type"),
            }
        }
    })
    .await
    .expect("timeout waiting for notification");

    assert_eq!(paid_amount, mint_amount.into());

    // 6. Mint the tokens
    let proofs = wallet
        .wait_and_mint_quote(
            mint_quote,
            SplitTarget::default(),
            None,
            Duration::from_secs(30),
        )
        .await
        .expect("Failed to mint");

    assert_eq!(proofs.total_amount().unwrap(), mint_amount.into());
    assert_eq!(wallet.total_balance().await.unwrap(), mint_amount.into());
}

#[cfg(feature = "payjoin-regtest")]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_payjoin_mint() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");
    let mint_url = get_mint_url_from_env();
    let mint_amount = 10_000_u64;
    let primer_wallet = Wallet::new(
        &mint_url,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    fund_wallet_with_onchain(&primer_wallet, &bitcoin_client, mint_amount)
        .await
        .expect("failed to prime receiver mint with an onchain UTXO");

    let quote = request_payjoin_mint_quote(&mint_url)
        .await
        .expect("mint should create a Payjoin-enabled quote");
    quote
        .payjoin
        .as_ref()
        .expect("Payjoin-enabled quote should include Payjoin params");

    send_payjoin_with_bitcoin_core(&bitcoin_client, &quote, mint_amount)
        .await
        .expect("Payjoin sender flow should complete");

    let mine_addr = bitcoin_client
        .get_new_address()
        .expect("Failed to get address");
    bitcoin_client
        .generate_blocks(&mine_addr, 1)
        .expect("Failed to mine block");

    let paid_quote = timeout(Duration::from_secs(60), async {
        loop {
            let quote = fetch_onchain_mint_quote(&mint_url, &quote.quote)
                .await
                .expect("fetch onchain mint quote");
            if quote.amount_paid >= mint_amount.into() {
                return quote;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .expect("timeout waiting for Payjoin mint quote to be paid");

    assert_eq!(paid_quote.quote, quote.quote);
    assert!(paid_quote.amount_paid >= mint_amount.into());
}

#[cfg(feature = "payjoin-regtest")]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_payjoin_melt_between_mints() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");
    let payer_mint_url = get_mint_url_from_env();
    let receiver_mint_url = get_second_mint_url_from_env();
    let payer_wallet = Wallet::new(
        &payer_mint_url,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create payer wallet");
    let receiver_wallet = Wallet::new(
        &receiver_mint_url,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create receiver wallet");

    fund_wallet_with_onchain(&payer_wallet, &bitcoin_client, 80_000)
        .await
        .expect("failed to fund payer wallet");
    fund_wallet_with_onchain(&receiver_wallet, &bitcoin_client, 10_000)
        .await
        .expect("failed to prime receiver mint with a Payjoin contribution UTXO");

    let melt_amount = 20_000_u64;
    let receiver_quote = request_payjoin_mint_quote(&receiver_mint_url)
        .await
        .expect("receiver mint should create a Payjoin-enabled quote");
    assert!(
        receiver_quote.payjoin.is_some(),
        "receiver quote should include Payjoin parameters"
    );

    let melt_quote_response =
        request_payjoin_melt_quote(&payer_mint_url, &receiver_quote, melt_amount)
            .await
            .expect("payer mint should accept Payjoin melt quote");
    assert!(
        melt_quote_response.payjoin.is_some(),
        "melt quote should confirm Payjoin acceptance"
    );

    let melt_quote = selected_onchain_melt_quote(&payer_wallet, &melt_quote_response)
        .expect("failed to select onchain melt quote");
    let melt_quote = payer_wallet
        .select_onchain_melt_quote(melt_quote)
        .await
        .expect("failed to persist selected melt quote");
    let prepared = payer_wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .expect("failed to prepare Payjoin melt");
    let mine_addr = bitcoin_client
        .get_new_address()
        .expect("Failed to get address");

    let melt_result = timeout(Duration::from_secs(180), async {
        let confirm_future = prepared.confirm();
        tokio::pin!(confirm_future);
        loop {
            tokio::select! {
                res = &mut confirm_future => {
                    return res.expect("failed to confirm Payjoin melt");
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();
                }
            }
        }
    })
    .await
    .expect("timeout waiting for Payjoin melt confirmation");

    assert_eq!(melt_result.state(), cdk::nuts::MeltQuoteState::Paid);

    let paid_receiver_quote = timeout(Duration::from_secs(60), async {
        loop {
            let quote = fetch_onchain_mint_quote(&receiver_mint_url, &receiver_quote.quote)
                .await
                .expect("fetch receiver onchain mint quote");
            if quote.amount_paid >= melt_amount.into() {
                return quote;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .expect("timeout waiting for receiver Payjoin mint quote to be paid");

    assert_eq!(paid_receiver_quote.quote, receiver_quote.quote);
    assert!(paid_receiver_quote.amount_paid >= melt_amount.into());
}

/// Drives a Payjoin melt through the async (poller-driven) send path and proves
/// the broadcast transaction actually batches a receiver-contributed input.
///
/// With the asynchronous send design, `make_payment` returns `Pending`
/// immediately and the mint's background poller posts the original PSBT,
/// receives the receiver mint's Payjoin proposal, and broadcasts the combined
/// transaction. We capture that transaction from the mempool *before* mining it
/// and assert it has at least two inputs: the payer mint's input plus at least
/// one input contributed by the receiver mint (the defining property of
/// Payjoin). A non-Payjoin fallback send from the payer mint would spend only
/// its own input(s).
#[cfg(feature = "payjoin-regtest")]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_payjoin_melt_batches_sender_and_receiver_inputs() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");
    let payer_mint_url = get_mint_url_from_env();
    let receiver_mint_url = get_second_mint_url_from_env();
    let payer_wallet = Wallet::new(
        &payer_mint_url,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create payer wallet");
    let receiver_wallet = Wallet::new(
        &receiver_mint_url,
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create receiver wallet");

    fund_wallet_with_onchain(&payer_wallet, &bitcoin_client, 80_000)
        .await
        .expect("failed to fund payer wallet");
    fund_wallet_with_onchain(&receiver_wallet, &bitcoin_client, 10_000)
        .await
        .expect("failed to prime receiver mint with a Payjoin contribution UTXO");

    let melt_amount = 20_000_u64;
    let receiver_quote = request_payjoin_mint_quote(&receiver_mint_url)
        .await
        .expect("receiver mint should create a Payjoin-enabled quote");
    assert!(
        receiver_quote.payjoin.is_some(),
        "receiver quote should include Payjoin parameters"
    );

    let melt_quote_response =
        request_payjoin_melt_quote(&payer_mint_url, &receiver_quote, melt_amount)
            .await
            .expect("payer mint should accept Payjoin melt quote");
    assert!(
        melt_quote_response.payjoin.is_some(),
        "melt quote should confirm Payjoin acceptance"
    );

    let melt_quote = selected_onchain_melt_quote(&payer_wallet, &melt_quote_response)
        .expect("failed to select onchain melt quote");
    let melt_quote = payer_wallet
        .select_onchain_melt_quote(melt_quote)
        .await
        .expect("failed to persist selected melt quote");
    let prepared = payer_wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .expect("failed to prepare Payjoin melt");

    // The onchain melt is asynchronous: `make_payment` returns immediately and
    // the background poller drives the Payjoin negotiation + broadcast.
    let outcome = prepared
        .confirm_prefer_async()
        .await
        .expect("failed to confirm Payjoin melt");
    let pending = match outcome {
        MeltOutcome::Pending(pending) => pending,
        MeltOutcome::Paid(_) => {
            panic!("onchain Payjoin melt must be pending, not immediately paid")
        }
    };

    // Capture the combined transaction from the mempool before mining it, and
    // assert it batches a receiver-contributed input.
    let input_count = timeout(Duration::from_secs(180), async {
        loop {
            if let Some(count) = bitcoin_client
                .mempool_tx_input_count_to_address(&receiver_quote.request)
                .expect("inspect mempool")
            {
                return count;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .expect("timeout waiting for the Payjoin transaction to reach the mempool");

    assert!(
        input_count >= 2,
        "Payjoin transaction must batch sender and receiver inputs, got {input_count} input(s)"
    );

    // Mine until the melt finalizes.
    let mine_addr = bitcoin_client
        .get_new_address()
        .expect("Failed to get address");
    let finalized = timeout(Duration::from_secs(120), async {
        let mut finalized_future = Box::pin(std::future::IntoFuture::into_future(pending));
        loop {
            tokio::select! {
                res = &mut finalized_future => break res.expect("failed to finalize Payjoin melt"),
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();
                }
            }
        }
    })
    .await
    .expect("timeout waiting for Payjoin melt to finalize");
    assert_eq!(finalized.state(), cdk::nuts::MeltQuoteState::Paid);

    // The receiver mint must be credited the melt amount.
    let paid_receiver_quote = timeout(Duration::from_secs(60), async {
        loop {
            let quote = fetch_onchain_mint_quote(&receiver_mint_url, &receiver_quote.quote)
                .await
                .expect("fetch receiver onchain mint quote");
            if quote.amount_paid >= melt_amount.into() {
                return quote;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .expect("timeout waiting for receiver Payjoin mint quote to be paid");

    assert_eq!(paid_receiver_quote.quote, receiver_quote.quote);
    assert!(paid_receiver_quote.amount_paid >= melt_amount.into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_melt() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 50_000;

    // 1. Fund the wallet via onchain mint
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .unwrap();

    bitcoin_client
        .send_to_address(&mint_quote.request, mint_amount)
        .expect("failed to send bitcoin");

    let mine_addr = bitcoin_client.get_new_address().unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    let mut subscription = wallet
        .subscribe(WalletSubscription::MintQuoteOnchainState(vec![mint_quote
            .id
            .clone()]))
        .await
        .expect("failed to subscribe");

    let mut paid_amount = cdk::amount::Amount::from(0);
    timeout(Duration::from_secs(30), async {
        while let Some(msg) = subscription.recv().await {
            match msg.into_inner() {
                NotificationPayload::MintQuoteOnchainResponse(response) => {
                    assert_eq!(response.quote, mint_quote.id);
                    if response.amount_paid == mint_amount.into() {
                        paid_amount = response.amount_paid;
                        return;
                    }
                }
                _ => panic!("Unexpected notification type"),
            }
        }
    })
    .await
    .expect("timeout waiting for notification");

    assert_eq!(paid_amount, mint_amount.into());

    wallet
        .wait_and_mint_quote(
            mint_quote,
            SplitTarget::default(),
            None,
            Duration::from_secs(60),
        )
        .await
        .unwrap();

    assert_eq!(wallet.total_balance().await.unwrap(), mint_amount.into());

    // 2. Request onchain melt options
    let dest_addr = bitcoin_client.get_new_address().unwrap();
    let melt_amount = 20_000;

    let melt_quotes = wallet
        .quote_onchain_melt_options(&dest_addr.to_string(), melt_amount.into(), None)
        .await
        .expect("Failed to get melt quotes");

    assert!(!melt_quotes.is_empty());
    let melt_quote = wallet
        .select_onchain_melt_quote(melt_quotes[0].clone())
        .await
        .expect("Failed to select melt quote");

    println!("Melt quote selected: {:?}", melt_quote);

    // 3. Prepare and confirm melt
    let prepared = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .expect("Failed to prepare melt");

    // We need to mine blocks for the transaction to confirm.
    // We will generate a block every second until the confirm future completes.
    let _melt_result = timeout(Duration::from_secs(60), async {
        let confirm_future = prepared.confirm();
        tokio::pin!(confirm_future);
        loop {
            tokio::select! {
                res = &mut confirm_future => {
                    return res.expect("Failed to confirm melt");
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();
                }
            }
        }
    })
    .await
    .expect("timeout waiting for melt confirmation");

    // Check balance
    let remaining_balance = wallet.total_balance().await.unwrap();
    // Balance should be mint_amount - melt_amount - fee
    assert!(remaining_balance < (mint_amount - melt_amount).into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_melt_selects_standard_fee_index() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 50_000;
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .unwrap();

    bitcoin_client
        .send_to_address(&mint_quote.request, mint_amount)
        .expect("failed to send bitcoin");

    let mine_addr = bitcoin_client.get_new_address().unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    wallet
        .wait_and_mint_quote(
            mint_quote,
            SplitTarget::default(),
            None,
            Duration::from_secs(60),
        )
        .await
        .unwrap();

    let dest_addr = bitcoin_client.get_new_address().unwrap();
    let melt_amount = 20_000;
    let melt_quotes = wallet
        .quote_onchain_melt_options(&dest_addr.to_string(), melt_amount.into(), None)
        .await
        .expect("Failed to get melt quotes");

    let standard_quote = melt_quotes
        .into_iter()
        .find(|quote| quote.fee_index == Some(1))
        .expect("expected Standard fee_index 1 option");
    let selected_quote = wallet
        .select_onchain_melt_quote(standard_quote.clone())
        .await
        .expect("Failed to select melt quote");

    assert_eq!(selected_quote.fee_index, Some(1));
    assert_eq!(
        selected_quote.estimated_blocks,
        standard_quote.estimated_blocks
    );
    assert_eq!(selected_quote.fee_reserve, standard_quote.fee_reserve);

    let prepared = wallet
        .prepare_melt(&selected_quote.id, std::collections::HashMap::new())
        .await
        .expect("Failed to prepare melt");

    timeout(Duration::from_secs(60), async {
        let confirm_future = prepared.confirm();
        tokio::pin!(confirm_future);
        loop {
            tokio::select! {
                res = &mut confirm_future => {
                    return res.expect("Failed to confirm melt");
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();
                }
            }
        }
    })
    .await
    .expect("timeout waiting for melt confirmation");

    let remaining_balance = wallet.total_balance().await.unwrap();
    assert!(remaining_balance < (mint_amount - melt_amount).into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_restore() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        seed,
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 20_000;

    // 1. Fund the wallet via onchain mint
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .unwrap();

    bitcoin_client
        .send_to_address(&mint_quote.request, mint_amount)
        .expect("failed to send bitcoin");

    let mine_addr = bitcoin_client.get_new_address().unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    wallet
        .wait_and_mint_quote(
            mint_quote,
            SplitTarget::default(),
            None,
            Duration::from_secs(60),
        )
        .await
        .unwrap();

    assert_eq!(wallet.total_balance().await.unwrap(), mint_amount.into());

    // 2. Create a new wallet instance with the same seed
    let wallet_2 = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        seed,
        None,
    )
    .expect("failed to create new wallet");

    assert_eq!(wallet_2.total_balance().await.unwrap(), 0.into());

    // 3. Restore the wallet
    let restored = wallet_2.restore().await.unwrap();
    assert_eq!(restored.unspent, mint_amount.into());
    assert_eq!(wallet_2.total_balance().await.unwrap(), mint_amount.into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_mint_multiple_payments() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let total_amount = 30_000;
    let payment_1 = 10_000;
    let payment_2 = 20_000;

    // 1. Request a mint quote for onchain payment
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(total_amount.into()),
            None,
            None,
        )
        .await
        .expect("Failed to get mint quote");

    // 2. Send first bitcoin payment
    bitcoin_client
        .send_to_address(&mint_quote.request, payment_1)
        .expect("Failed to send bitcoin");

    let mine_addr = bitcoin_client.get_new_address().unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    // 3. Send second bitcoin payment
    bitcoin_client
        .send_to_address(&mint_quote.request, payment_2)
        .expect("Failed to send bitcoin");

    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    let mut stream = wallet.proof_stream(mint_quote, SplitTarget::default(), None);
    let mut proofs = Proofs::new();

    while proofs.total_amount().unwrap() < total_amount.into() {
        if let Some(Ok(p)) = stream.next().await {
            proofs.extend(p);
        }
    }

    assert_eq!(proofs.total_amount().unwrap(), total_amount.into());
    assert_eq!(wallet.total_balance().await.unwrap(), total_amount.into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_melt_prefer_async() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 50_000;

    // Fund wallet
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .unwrap();

    bitcoin_client
        .send_to_address(&mint_quote.request, mint_amount)
        .unwrap();

    let mine_addr = bitcoin_client.get_new_address().unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    wallet
        .wait_and_mint_quote(
            mint_quote,
            SplitTarget::default(),
            None,
            Duration::from_secs(60),
        )
        .await
        .unwrap();

    // 2. Request onchain melt
    let dest_addr = bitcoin_client.get_new_address().unwrap();
    let melt_amount = 20_000;

    let melt_quotes = wallet
        .quote_onchain_melt_options(&dest_addr.to_string(), melt_amount.into(), None)
        .await
        .unwrap();

    let melt_quote = wallet
        .select_onchain_melt_quote(melt_quotes[0].clone())
        .await
        .unwrap();

    // 3. Confirm with prefer async
    let prepared = wallet
        .prepare_melt(&melt_quote.id, std::collections::HashMap::new())
        .await
        .unwrap();

    let outcome = prepared.confirm_prefer_async().await.unwrap();

    match outcome {
        MeltOutcome::Pending(pending) => {
            // We need to mine blocks for the transaction to confirm.
            // We will generate a block every second until the finalized future completes.
            let finalized = timeout(Duration::from_secs(60), async {
                let mut finalized_future = Box::pin(std::future::IntoFuture::into_future(pending));
                loop {
                    tokio::select! {
                        res = &mut finalized_future => break res.unwrap(),
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {
                            let mine_addr = bitcoin_client.get_new_address().unwrap();
                            bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();
                        }
                    }
                }
            })
            .await
            .expect("Melt timed out");

            assert_eq!(finalized.state(), cdk::nuts::MeltQuoteState::Paid);
        }
        MeltOutcome::Paid(_) => panic!("Expected pending outcome for onchain melt"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_mint_underpaid() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let quote_amount = 20_000;
    let actual_paid = 15_000;

    // 1. Request a mint quote
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(quote_amount.into()),
            None,
            None,
        )
        .await
        .unwrap();

    // 2. Underpay the quote
    bitcoin_client
        .send_to_address(&mint_quote.request, actual_paid)
        .unwrap();

    let mine_addr = bitcoin_client.get_new_address().unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    // 3. Mint what was paid
    // Let's poll until amount_paid is correct
    let mut quote = mint_quote.clone();
    for _ in 0..30 {
        quote = wallet.check_mint_quote_status(&quote.id).await.unwrap();
        if quote.amount_paid >= actual_paid.into() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    assert!(quote.amount_paid >= actual_paid.into());

    let proofs = wallet
        .mint(&quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    assert_eq!(proofs.total_amount().unwrap(), actual_paid.into());
    assert_eq!(wallet.total_balance().await.unwrap(), actual_paid.into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_mint_unique_addresses() {
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 10_000;

    // 1. Request first mint quote
    let mint_quote_1 = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .expect("Failed to get first mint quote");

    // 2. Request second mint quote
    let mint_quote_2 = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .expect("Failed to get second mint quote");

    // 3. Verify addresses are unique
    assert_ne!(
        mint_quote_1.request, mint_quote_2.request,
        "Mint quotes should have unique addresses"
    );

    assert!(mint_quote_1.request.starts_with("bcrt1"));
    assert!(mint_quote_2.request.starts_with("bcrt1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_concurrent_mint_quotes() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount_1 = 10_000;
    let mint_amount_2 = 20_000;
    let mint_amount_3 = 30_000;

    // Request 3 quotes concurrently
    let (quote_1, quote_2, quote_3) = tokio::try_join!(
        wallet.mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount_1.into()),
            None,
            None,
        ),
        wallet.mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount_2.into()),
            None,
            None,
        ),
        wallet.mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount_3.into()),
            None,
            None,
        ),
    )
    .expect("Failed to get mint quotes");

    // Pay all 3
    bitcoin_client
        .send_to_address(&quote_1.request, mint_amount_1)
        .expect("failed to send bitcoin 1");
    bitcoin_client
        .send_to_address(&quote_2.request, mint_amount_2)
        .expect("failed to send bitcoin 2");
    bitcoin_client
        .send_to_address(&quote_3.request, mint_amount_3)
        .expect("failed to send bitcoin 3");

    // Mine 1 block to confirm all 3 transactions together
    let mine_addr = bitcoin_client.get_new_address().unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    // Mint all 3 concurrently
    let (proofs_1, proofs_2, proofs_3) = tokio::try_join!(
        wallet.wait_and_mint_quote(
            quote_1,
            SplitTarget::default(),
            None,
            Duration::from_secs(30),
        ),
        wallet.wait_and_mint_quote(
            quote_2,
            SplitTarget::default(),
            None,
            Duration::from_secs(30),
        ),
        wallet.wait_and_mint_quote(
            quote_3,
            SplitTarget::default(),
            None,
            Duration::from_secs(30),
        ),
    )
    .expect("Failed to mint concurrently");

    assert_eq!(proofs_1.total_amount().unwrap(), mint_amount_1.into());
    assert_eq!(proofs_2.total_amount().unwrap(), mint_amount_2.into());
    assert_eq!(proofs_3.total_amount().unwrap(), mint_amount_3.into());

    let total_expected = mint_amount_1 + mint_amount_2 + mint_amount_3;
    assert_eq!(wallet.total_balance().await.unwrap(), total_expected.into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_concurrent_melt_quotes() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 200_000;

    // 1. Fund the wallet with a large onchain mint
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .unwrap();

    bitcoin_client
        .send_to_address(&mint_quote.request, mint_amount)
        .expect("failed to send bitcoin");

    let mine_addr = bitcoin_client.get_new_address().unwrap();
    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();

    wallet
        .wait_and_mint_quote(
            mint_quote,
            SplitTarget::default(),
            None,
            Duration::from_secs(60),
        )
        .await
        .unwrap();

    assert_eq!(wallet.total_balance().await.unwrap(), mint_amount.into());

    // 2. Prepare 3 destinations and request options
    let dest_1 = bitcoin_client.get_new_address().unwrap();
    let dest_2 = bitcoin_client.get_new_address().unwrap();
    let dest_3 = bitcoin_client.get_new_address().unwrap();

    let melt_amount_1 = 20_000;
    let melt_amount_2 = 30_000;
    let melt_amount_3 = 40_000;

    // Request options sequentially because quote creation isn't the complex state mutation
    let options_1 = wallet
        .quote_onchain_melt_options(&dest_1.to_string(), melt_amount_1.into(), None)
        .await
        .unwrap();
    let options_2 = wallet
        .quote_onchain_melt_options(&dest_2.to_string(), melt_amount_2.into(), None)
        .await
        .unwrap();
    let options_3 = wallet
        .quote_onchain_melt_options(&dest_3.to_string(), melt_amount_3.into(), None)
        .await
        .unwrap();

    let melt_quote_1 = wallet
        .select_onchain_melt_quote(options_1[0].clone())
        .await
        .unwrap();
    let melt_quote_2 = wallet
        .select_onchain_melt_quote(options_2[0].clone())
        .await
        .unwrap();
    let melt_quote_3 = wallet
        .select_onchain_melt_quote(options_3[0].clone())
        .await
        .unwrap();

    // 3. Prepare melts concurrently to stress input selection
    let (prep_1, prep_2, prep_3) = tokio::try_join!(
        wallet.prepare_melt(&melt_quote_1.id, std::collections::HashMap::new()),
        wallet.prepare_melt(&melt_quote_2.id, std::collections::HashMap::new()),
        wallet.prepare_melt(&melt_quote_3.id, std::collections::HashMap::new()),
    )
    .expect("Failed to prepare melts concurrently");

    // 4. Confirm concurrently
    timeout(Duration::from_secs(120), async {
        let conf_1 = prep_1.confirm();
        let conf_2 = prep_2.confirm();
        let conf_3 = prep_3.confirm();

        tokio::pin!(conf_1);
        tokio::pin!(conf_2);
        tokio::pin!(conf_3);

        let mut confirmed_1 = false;
        let mut confirmed_2 = false;
        let mut confirmed_3 = false;

        loop {
            if confirmed_1 && confirmed_2 && confirmed_3 {
                break;
            }

            tokio::select! {
                res = &mut conf_1, if !confirmed_1 => {
                    res.expect("Failed conf 1");
                    confirmed_1 = true;
                }
                res = &mut conf_2, if !confirmed_2 => {
                    res.expect("Failed conf 2");
                    confirmed_2 = true;
                }
                res = &mut conf_3, if !confirmed_3 => {
                    res.expect("Failed conf 3");
                    confirmed_3 = true;
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    bitcoin_client.generate_blocks(&mine_addr, 1).unwrap();
                }
            }
        }
    })
    .await
    .expect("timeout waiting for multiple melts");

    // Balance should be reduced by the melts and their fees
    let final_balance = wallet.total_balance().await.unwrap();
    let total_melted = melt_amount_1 + melt_amount_2 + melt_amount_3;
    assert!(final_balance < (mint_amount - total_melted).into());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_mint_unissued_quotes_onchain() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");
    let localstore = Arc::new(memory::empty().await.unwrap());
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        localstore.clone(),
        seed,
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 10_000;

    // 1. Request a mint quote for onchain payment
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .expect("Failed to get mint quote");

    // Verify the quote is in unissued quotes before payment
    let unissued_before = wallet.get_unissued_mint_quotes().await.unwrap();
    assert!(
        unissued_before.iter().any(|q| q.id == mint_quote.id),
        "Onchain quote should be in unissued quotes before payment"
    );

    // Send bitcoin to the mint address
    bitcoin_client
        .send_to_address(&mint_quote.request, mint_amount)
        .expect("Failed to send bitcoin");

    // Mine a block to confirm the transaction
    let mine_addr = bitcoin_client
        .get_new_address()
        .expect("Failed to get address");
    bitcoin_client
        .generate_blocks(&mine_addr, 1)
        .expect("Failed to mine block");

    // Wait for payment to be recognized
    wallet
        .wait_for_payment(&mint_quote, tokio::time::Duration::from_secs(30))
        .await
        .unwrap();

    // Verify initial balance is zero
    assert_eq!(
        wallet.total_balance().await.unwrap(),
        cdk::amount::Amount::ZERO
    );

    // Call mint_unissued_quotes - this should mint the paid quote
    let total_minted = wallet.mint_unissued_quotes().await.unwrap();

    // Verify the amount minted is correct
    assert_eq!(
        total_minted,
        cdk::amount::Amount::from(mint_amount),
        "mint_unissued_quotes should have minted the onchain quote"
    );

    // Verify wallet balance matches
    assert_eq!(
        wallet.total_balance().await.unwrap(),
        cdk::amount::Amount::from(mint_amount)
    );

    // Calling mint_unissued_quotes again should return 0 (quote already fully issued)
    let second_check = wallet.mint_unissued_quotes().await.unwrap();
    assert_eq!(
        second_check,
        cdk::amount::Amount::ZERO,
        "Second check should return 0 as quote is fully issued"
    );

    let restarted_wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        localstore,
        seed,
        None,
    )
    .expect("failed to recreate wallet with same localstore and seed");

    assert_eq!(
        restarted_wallet.total_balance().await.unwrap(),
        cdk::amount::Amount::from(mint_amount),
        "Restarted wallet should retain minted proofs"
    );
    let restart_check = restarted_wallet.mint_unissued_quotes().await.unwrap();
    assert_eq!(
        restart_check,
        cdk::amount::Amount::ZERO,
        "Restarted wallet should not remint an already-issued onchain quote"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_check_all_mint_quotes_onchain() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 10_000;

    // 1. Request a mint quote for onchain payment
    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .expect("Failed to get mint quote");

    // Verify the quote is in unissued quotes before payment
    let unissued_before = wallet.get_unissued_mint_quotes().await.unwrap();
    assert!(
        unissued_before.iter().any(|q| q.id == mint_quote.id),
        "Onchain quote should be in unissued quotes before payment"
    );

    let mut subscription = wallet
        .subscribe(WalletSubscription::MintQuoteOnchainState(vec![mint_quote
            .id
            .clone()]))
        .await
        .expect("failed to subscribe");

    // Send bitcoin to the mint address
    bitcoin_client
        .send_to_address(&mint_quote.request, mint_amount)
        .expect("Failed to send bitcoin");

    // Mine a block to confirm the transaction
    let mine_addr = bitcoin_client
        .get_new_address()
        .expect("Failed to get address");
    bitcoin_client
        .generate_blocks(&mine_addr, 1)
        .expect("Failed to mine block");

    // Poll until paid
    let mut paid_amount = cdk::amount::Amount::from(0);
    timeout(Duration::from_secs(30), async {
        while let Some(msg) = subscription.recv().await {
            match msg.into_inner() {
                NotificationPayload::MintQuoteOnchainResponse(response) => {
                    assert_eq!(response.quote, mint_quote.id);
                    if response.amount_paid == mint_amount.into() {
                        paid_amount = response.amount_paid;
                        return;
                    }
                }
                _ => panic!("Unexpected notification type"),
            }
        }
    })
    .await
    .expect("timeout waiting for notification");
    assert_eq!(paid_amount, cdk::amount::Amount::from(mint_amount));

    // Verify initial balance is zero
    assert_eq!(
        wallet.total_balance().await.unwrap(),
        cdk::amount::Amount::ZERO
    );

    // Call mint_unissued_quotes - this should mint the paid quote
    let total_minted = wallet.mint_unissued_quotes().await.unwrap();

    // Verify the amount minted is correct
    assert_eq!(
        total_minted,
        cdk::amount::Amount::from(mint_amount),
        "mint_unissued_quotes should have minted the onchain quote"
    );

    // Verify wallet balance matches
    assert_eq!(
        wallet.total_balance().await.unwrap(),
        cdk::amount::Amount::from(mint_amount)
    );

    // Calling mint_unissued_quotes again should return 0 (quote already fully issued)
    let second_check = wallet.mint_unissued_quotes().await.unwrap();
    assert_eq!(
        second_check,
        cdk::amount::Amount::ZERO,
        "Second check should return 0 as quote is fully issued"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_quote_amount_issued_tracking() {
    let bitcoin_client = init_bitcoin_client().expect("Failed to init bitcoin client");

    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 10_000;

    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .expect("Failed to get mint quote");

    assert_eq!(mint_quote.amount_paid, cdk::amount::Amount::from(0));
    assert_eq!(mint_quote.amount_issued, cdk::amount::Amount::from(0));

    let mut subscription = wallet
        .subscribe(WalletSubscription::MintQuoteOnchainState(vec![mint_quote
            .id
            .clone()]))
        .await
        .expect("failed to subscribe");

    bitcoin_client
        .send_to_address(&mint_quote.request, mint_amount)
        .expect("Failed to send bitcoin");

    let mine_addr = bitcoin_client
        .get_new_address()
        .expect("Failed to get address");
    bitcoin_client
        .generate_blocks(&mine_addr, 1)
        .expect("Failed to mine block");

    timeout(Duration::from_secs(30), async {
        while let Some(msg) = subscription.recv().await {
            match msg.into_inner() {
                NotificationPayload::MintQuoteOnchainResponse(response) => {
                    assert_eq!(response.quote, mint_quote.id);
                    if response.amount_paid == mint_amount.into() {
                        return;
                    }
                }
                _ => panic!("Unexpected notification type"),
            }
        }
    })
    .await
    .expect("timeout waiting for notification");

    let quote_after_payment = wallet
        .check_mint_quote_status(&mint_quote.id)
        .await
        .unwrap();
    assert_eq!(
        quote_after_payment.amount_paid,
        cdk::amount::Amount::from(mint_amount)
    );
    assert_eq!(
        quote_after_payment.amount_issued,
        cdk::amount::Amount::from(0)
    );

    wallet
        .mint(&mint_quote.id, SplitTarget::default(), None)
        .await
        .unwrap();

    let quote_after_mint = wallet
        .check_mint_quote_status(&mint_quote.id)
        .await
        .unwrap();
    assert_eq!(
        quote_after_mint.amount_paid,
        cdk::amount::Amount::from(mint_amount)
    );
    assert_eq!(
        quote_after_mint.amount_issued,
        cdk::amount::Amount::from(mint_amount)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_onchain_attempt_to_mint_unpaid() {
    let wallet = Wallet::new(
        &get_mint_url_from_env(),
        CurrencyUnit::Sat,
        Arc::new(memory::empty().await.unwrap()),
        Mnemonic::generate(12).unwrap().to_seed_normalized(""),
        None,
    )
    .expect("failed to create new wallet");

    let mint_amount = 10_000;

    let mint_quote = wallet
        .mint_quote(
            PaymentMethod::from_str("onchain").unwrap(),
            Some(mint_amount.into()),
            None,
            None,
        )
        .await
        .expect("Failed to get mint quote");

    let active_keyset = wallet.active_keyset().await.unwrap();
    let fee_and_amounts = (0, ((0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>())).into();
    let premint_secrets = cdk::nuts::PreMintSecrets::random(
        active_keyset.id,
        mint_amount.into(),
        &cdk::amount::SplitTarget::default(),
        &fee_and_amounts,
    )
    .unwrap();

    let request = cdk::nuts::MintRequest {
        quote: mint_quote.id.clone(),
        outputs: premint_secrets.blinded_messages(),
        signature: None,
    };

    let response = cdk::wallet::HttpClient::new(get_mint_url_from_env().parse().unwrap(), None)
        .post_mint(&PaymentMethod::from_str("onchain").unwrap(), request)
        .await;

    assert!(response.is_err());
    let err = response.unwrap_err();
    match err {
        cdk::error::Error::UnpaidQuote => {} // Pass
        _ => panic!("Expected UnpaidQuote error, got {:?}", err),
    }
}
