use std::sync::Arc;
use std::time::Duration;

use bitcoin::hashes::{sha256, Hash};
use bitcoin::hex::prelude::FromHex;
use bitcoin::secp256k1::Secp256k1;
use cdk::error::Error;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, SecretKey};
use cdk::wallet::Wallet;
use cdk::Amount;
use cdk_sqlite::wallet::memory;
use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
use rand::Rng;

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Initialize the memory store for the wallet
    let localstore = memory::empty().await?;

    // Generate a random seed for the wallet
    let seed = rand::rng().random::<[u8; 64]>();

    // Define the mint URL and currency unit
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(10);

    // Create a new wallet
    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None)?;

    let quote = wallet.mint_quote(amount, None).await?;
    let proofs = wallet
        .wait_and_mint_quote(
            quote,
            Default::default(),
            Default::default(),
            Duration::from_secs(10),
        )
        .await?;

    let receive_amount = proofs.total_amount()?;
    println!("Received {} from mint {}", receive_amount, mint_url);

    // Now melt what we have
    // We need to prepare a lightning invoice
    let private_key = SecretKey::from_slice(
        &<[u8; 32]>::from_hex("e126f68f7eafcc8b74f54d269fe206be715000f94dac067d1c04a8ca3b2db734")
            .unwrap(),
    )
    .unwrap();
    let random_bytes = rand::rng().random::<[u8; 32]>();
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
