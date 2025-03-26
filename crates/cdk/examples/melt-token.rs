use std::sync::Arc;

use bitcoin::hashes::{sha256, Hash};
use bitcoin::hex::prelude::FromHex;
use bitcoin::secp256k1::Secp256k1;
use cdk::amount::SplitTarget;
use cdk::error::Error;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MintQuoteState, NotificationPayload, SecretKey};
use cdk::wallet::{Wallet, WalletSubscription};
use cdk::Amount;
use cdk_sqlite::wallet::memory;
use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
use rand::Rng;

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Initialize the memory store for the wallet
    let localstore = memory::empty().await?;

    // Generate a random seed for the wallet
    let seed = rand::thread_rng().gen::<[u8; 32]>();

    // Define the mint URL and currency unit
    let mint_url = "https://testnut.cashu.space";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(10);

    // Create a new wallet
    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None)?;

    // Request a mint quote from the wallet
    let quote = wallet.mint_quote(amount, None).await?;
    println!("Quote: {:#?}", quote);

    // Subscribe to updates on the mint quote state
    let mut subscription = wallet
        .subscribe(WalletSubscription::Bolt11MintQuoteState(vec![quote
            .id
            .clone()]))
        .await;

    // Wait for the mint quote to be paid
    while let Some(msg) = subscription.recv().await {
        if let NotificationPayload::MintQuoteBolt11Response(response) = msg {
            if response.state == MintQuoteState::Paid {
                break;
            }
        }
    }

    // Mint the received amount
    let proofs = wallet.mint(&quote.id, SplitTarget::default(), None).await?;
    let receive_amount = proofs.total_amount()?;
    println!("Received {} from mint {}", receive_amount, mint_url);

    // Now melt what we have
    // We need to prepare a lightning invoice
    let private_key = SecretKey::from_slice(
        &<[u8; 32]>::from_hex("e126f68f7eafcc8b74f54d269fe206be715000f94dac067d1c04a8ca3b2db734")
            .unwrap(),
    )
    .unwrap();
    let mut random_bytes = [1u8; 32];
    rand::thread_rng().fill(&mut random_bytes);
    let payment_hash = sha256::Hash::from_slice(&random_bytes).unwrap();
    let payment_secret = PaymentSecret([42u8; 32]);
    let invoice_to_be_paid = InvoiceBuilder::new(Currency::Bitcoin)
        .amount_milli_satoshis(5 * 1000)
        .description("Pay me".into())
        .payment_hash(payment_hash)
        .payment_secret(payment_secret)
        .current_timestamp()
        .min_final_cltv_expiry_delta(144)
        .build_signed(|hash| Secp256k1::new().sign_ecdsa_recoverable(hash, &private_key))
        .unwrap()
        .to_string();
    println!("Invoice to be paid: {}", invoice_to_be_paid);

    let melt_quote = wallet.melt_quote(invoice_to_be_paid, None).await?;
    println!(
        "Melt quote: {} {} {:?}",
        melt_quote.amount, melt_quote.state, melt_quote,
    );

    let melted = wallet.melt(&melt_quote.id).await?;
    println!("Melted: {:?}", melted);

    Ok(())
}
